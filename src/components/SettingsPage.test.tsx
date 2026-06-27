import { invoke } from "@tauri-apps/api/core";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { AppSettings, AudioSourceInfo, ProviderReadiness } from "../types";
import SettingsPage from "./SettingsPage";
import {
  buildAwsCredentialSource,
  endpointCredentialKey,
  initialSettingsState,
  isCerebrasEndpoint,
  type SettingsState,
  setField,
  settingsReducer,
} from "./settingsTypes";
import "../i18n";

const mockedInvoke = vi.mocked(invoke);

function failPlaintextCredentialLoadback(args?: unknown): never {
  void args;
  throw new Error(
    "load_credential_cmd should not be invoked by frontend tests; use load_credential_presence_cmd and provider readiness instead.",
  );
}

// A minimal AppSettings fixture that hydrates the reducer into a known state.
// We lean on the `log_level` + `audio_settings` fields since those are what
// the HYDRATE_FROM_SETTINGS + AWS credential-load side effects key off of.
const baseSettings: AppSettings = {
  asr_provider: { type: "local_whisper" },
  tts_provider: { type: "none" },
  speak_aloud: false,
  whisper_model: "ggml-small.en.bin",
  llm_provider: {
    type: "api",
    endpoint: "http://localhost:11434/v1",
    api_key: "",
    model: "llama3.2",
  },
  llm_api_config: null,
  audio_settings: { sample_rate: 48000, channels: 2 },
  gemini: {
    auth: { type: "api_key", api_key: "" },
    model: "gemini-3.1-flash-live-preview",
  },
  log_level: "info",
};

const auraVoiceCatalogFixture = () =>
  Array.from({ length: 12 }, (_, index) => {
    const id = index === 0 ? "aura-asteria-en" : `aura-fixture-${index}-en`;
    return {
      id,
      display_name: index === 0 ? "Asteria" : `Fixture ${index}`,
      is_default: index === 0,
    };
  });

const selectedSystemSource = (): AudioSourceInfo => ({
  id: "system-default",
  name: "System audio",
  source_type: { type: "SystemDefault" },
  capture_target: "system-default",
  device_kind: null,
  is_default: true,
  supported_formats: [],
  default_format: null,
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
  is_active: false,
});

function resetStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    settings: baseSettings,
    audioSources: [selectedSystemSource()],
    selectedSourceIds: ["system-default"],
    sourceRecoveryIntent: null,
    models: [],
    modelStatus: null,
    settingsLoading: false,
    isDownloading: false,
    downloadProgress: null,
    isDeletingModel: null,
    nativeS2sEnabled: false,
    conversationMode: "notes",
    converseEngine: "pipelined",
    closeSettings: vi.fn(),
    saveSettings: vi.fn(async () => {}),
    downloadModel: vi.fn(),
    deleteModel: vi.fn(),
    listAwsProfiles: vi.fn(async () => []),
    ...overrides,
  });
}

describe("settingsReducer", () => {
  it("uses backend-aligned stereo capture defaults", () => {
    expect(initialSettingsState.audioSampleRate).toBe(48000);
    expect(initialSettingsState.audioChannels).toBe(2);
    expect(initialSettingsState.diarizationMode).toBe("provider");
    expect(initialSettingsState.diarizationSpeakerCount).toBe("auto");
    expect(initialSettingsState.diarizationMaxSpeakers).toBe(0);
  });

  it("SET_FIELD updates a single scalar field", () => {
    const next = settingsReducer(
      initialSettingsState,
      setField("logLevel", "debug"),
    );
    expect(next.logLevel).toBe("debug");
    // Other fields must be untouched.
    expect(next.audioSampleRate).toBe(initialSettingsState.audioSampleRate);
  });

  it("HYDRATE_FROM_SETTINGS collapses a partial patch into state in one dispatch", () => {
    const patch: Partial<SettingsState> = {
      asrType: "deepgram",
      deepgramApiKey: "dg-xyz",
      llmType: "aws_bedrock",
      awsBedrockRegion: "eu-west-1",
      logLevel: "trace",
    };
    const next = settingsReducer(initialSettingsState, {
      type: "HYDRATE_FROM_SETTINGS",
      patch,
    });
    expect(next.asrType).toBe("deepgram");
    expect(next.deepgramApiKey).toBe("dg-xyz");
    expect(next.llmType).toBe("aws_bedrock");
    expect(next.awsBedrockRegion).toBe("eu-west-1");
    expect(next.logLevel).toBe("trace");
  });

  it("HYDRATE_FROM_SETTINGS preserves a typed API key it does not patch (BUG-2 regression)", () => {
    // The IPC `settings` object is always redacted (skip_serializing), so the
    // settings-hydration path must NOT include `geminiApiKey` in its patch — the
    // credential store is the sole source for it. A patch that omits the field
    // must leave a user-typed value intact; if hydration ever re-seeds it from
    // the redacted settings (`geminiApiKey: ""`), the field blanks after Save.
    const typed: SettingsState = {
      ...initialSettingsState,
      geminiApiKey: "AIza-user-typed-key",
    };
    const next = settingsReducer(typed, {
      type: "HYDRATE_FROM_SETTINGS",
      // Mirrors the post-fix hydration patch: model + auth mode, but NO api key.
      patch: {
        geminiModel: "gemini-2.0-flash-live-001",
        geminiAuthMode: "api_key",
      },
    });
    expect(next.geminiApiKey).toBe("AIza-user-typed-key");
    expect(next.geminiModel).toBe("gemini-2.0-flash-live-001");
  });

  it("SET_AWS_SHARED_SECRET mirrors the secret into both ASR and Bedrock slots", () => {
    const next = settingsReducer(initialSettingsState, {
      type: "SET_AWS_SHARED_SECRET",
      secret: "shh",
    });
    expect(next.awsAsrSecretKey).toBe("shh");
    expect(next.awsBedrockSecretKey).toBe("shh");
  });

  it("CLEAR_AWS_SHARED_KEYS wipes secret + session on both ASR and Bedrock", () => {
    const seeded: SettingsState = {
      ...initialSettingsState,
      awsAsrAccessKey: "ak-a",
      awsBedrockAccessKey: "ak-b",
      awsAsrSecretKey: "a",
      awsBedrockSecretKey: "b",
      awsAsrSessionToken: "c",
      awsBedrockSessionToken: "d",
    };
    const next = settingsReducer(seeded, { type: "CLEAR_AWS_SHARED_KEYS" });
    expect(next.awsAsrAccessKey).toBe("");
    expect(next.awsBedrockAccessKey).toBe("");
    expect(next.awsAsrSecretKey).toBe("");
    expect(next.awsBedrockSecretKey).toBe("");
    expect(next.awsAsrSessionToken).toBe("");
    expect(next.awsBedrockSessionToken).toBe("");
  });

  it("buildAwsCredentialSource returns the right tagged union for each mode", () => {
    expect(buildAwsCredentialSource("default_chain", "", "")).toEqual({
      type: "default_chain",
    });
    expect(buildAwsCredentialSource("profile", "dev", "")).toEqual({
      type: "profile",
      name: "dev",
    });
    expect(buildAwsCredentialSource("access_keys", "", "AKIA")).toEqual({
      type: "access_keys",
      access_key: "AKIA",
    });
  });

  it("endpointCredentialKey routes known OpenAI-compatible endpoints", () => {
    expect(endpointCredentialKey("https://api.openai.com/v1")).toBe(
      "openai_api_key",
    );
    expect(endpointCredentialKey("https://api.cerebras.ai/v1")).toBe(
      "cerebras_api_key",
    );
    expect(endpointCredentialKey("https://api.cerebras.ai/v1/")).toBe(
      "cerebras_api_key",
    );
    expect(isCerebrasEndpoint("https://api.cerebras.ai/v1/")).toBe(true);
    expect(endpointCredentialKey("https://api.cerebras.ai/v1beta")).toBe(
      "openai_api_key",
    );
    expect(endpointCredentialKey("https://openrouter.ai/api/v1")).toBe(
      "openrouter_api_key",
    );
    expect(endpointCredentialKey("https://api.groq.com/openai/v1")).toBe(
      "groq_api_key",
    );
    expect(endpointCredentialKey("https://api.together.xyz/v1")).toBe(
      "together_api_key",
    );
    expect(endpointCredentialKey("https://api.fireworks.ai/inference/v1")).toBe(
      "fireworks_api_key",
    );
    expect(
      endpointCredentialKey(
        "https://generativelanguage.googleapis.com/v1beta/openai",
      ),
    ).toBe("gemini_api_key");
  });
});

describe("SettingsPage", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    mockedInvoke.mockImplementation(async (cmd: string) => {
      // Saved-key state comes from credential presence/readiness, never
      // plaintext credential loadback.
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
      if (cmd === "load_credential_presence_cmd") return [];
      if (cmd === "get_provider_readiness_cmd") return [];
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });
    resetStore();
  });

  // Settings sections are grouped into tabs; click one to reveal its
  // section before interacting with that section's fields.
  const goToTab = (name: RegExp) =>
    fireEvent.click(screen.getByRole("tab", { name }));

  const openSettingsAdvancedControls = (scope: HTMLElement) => {
    const summary = within(scope).getByText(/advanced provider controls/i, {
      selector: "summary",
    });
    const details = summary.closest("details") as HTMLDetailsElement;
    expect(details).toBeTruthy();
    expect(details.open).toBe(false);
    fireEvent.click(summary);
    expect(details.open).toBe(true);
    return details;
  };

  const saveCredentialCalls = () =>
    mockedInvoke.mock.calls.filter(([cmd]) => cmd === "save_credential_cmd");
  const providerReadinessCalls = () =>
    mockedInvoke.mock.calls.filter(
      ([cmd]) => cmd === "get_provider_readiness_cmd",
    );
  const notesReadinessArgs = (force?: boolean) => ({
    refresh: true,
    ...(force === undefined ? {} : { force }),
    conversationMode: "notes",
    converseEngine: "pipelined",
    requestId: expect.any(String),
  });
  const credentialHealthRowForKey = (key: string): HTMLElement => {
    const row = screen
      .getAllByText(key)
      .map((node) => node.closest(".settings-credential-health__item"))
      .find((node): node is HTMLElement => node instanceof HTMLElement);
    if (!row) throw new Error(`${key} credential row not found`);
    return row;
  };
  const readinessRowForProvider = async (
    name: RegExp,
  ): Promise<HTMLElement> => {
    return await waitFor(() => {
      const row = Array.from(
        document.querySelectorAll<HTMLElement>(".settings-readiness__item"),
      ).find((candidate) => {
        const label = candidate.querySelector(
          ".settings-readiness__provider",
        )?.textContent;
        return label ? name.test(label) : false;
      });
      if (!(row instanceof HTMLElement)) {
        throw new Error(`Readiness row not found for ${name}`);
      }
      return row;
    });
  };
  const modeOverviewCard = async (name: RegExp): Promise<HTMLElement> => {
    const overview = await screen.findByRole("region", {
      name: /product mode overview/i,
    });
    const heading = within(overview).getByRole("heading", {
      name,
      level: 4,
    });
    const card = heading.closest(".settings-mode-card");
    if (!(card instanceof HTMLElement)) {
      throw new Error(`Mode overview card not found for ${name}`);
    }
    return card;
  };
  const capabilityCardForProvider = async (
    name: RegExp,
  ): Promise<HTMLElement> => {
    const overview = await screen.findByRole("region", {
      name: /provider capability overview/i,
    });
    const heading = within(overview).getByRole("heading", {
      name,
      level: 5,
    });
    const card = heading.closest(".settings-provider-capability-card");
    if (!(card instanceof HTMLElement)) {
      throw new Error(`Provider capability card not found for ${name}`);
    }
    return card;
  };
  const settingsSectionForHeading = (name: RegExp): HTMLElement => {
    const section = screen
      .getByRole("heading", { name, level: 3 })
      .closest(".settings-section");
    if (!(section instanceof HTMLElement)) {
      throw new Error(`Settings section not found for ${name}`);
    }
    return section;
  };
  // A provider section contains two live regions (role="status"): the provider
  // readiness panel and the model catalog picker's announcement span. Scope to
  // the readiness panel (status message + privacy metadata) so the query is
  // unambiguous.
  const readinessStatus = (scope: HTMLElement): HTMLElement => {
    const panel = scope.querySelector(".settings-provider-readiness");
    if (!(panel instanceof HTMLElement)) {
      throw new Error("Provider readiness panel not found in scope");
    }
    return panel;
  };
  const openCredentialInput = async (
    scope: HTMLElement,
    label: RegExp,
    actionName: RegExp = /add key|replace key/i,
  ): Promise<HTMLInputElement> => {
    // Credential action buttons carry an aria-label that includes the field
    // label (e.g. "Replace key: Deepgram API key") for screen-reader context,
    // so scope the label lookup to the actual <input> to avoid matching both.
    const existing = within(scope).queryByLabelText(label, {
      selector: "input",
    });
    if (existing instanceof HTMLInputElement) return existing;
    fireEvent.click(within(scope).getByRole("button", { name: actionName }));
    const input = await within(scope).findByLabelText(label, {
      selector: "input",
    });
    if (!(input instanceof HTMLInputElement)) {
      throw new Error(`Credential input not found for ${label}`);
    }
    return input;
  };
  const clickSaveSettings = async () => {
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
    });
  };

  it("renders the settings dialog header + Save footer button", () => {
    render(<SettingsPage />);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: /^settings$/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /save settings/i }),
    ).toBeInTheDocument();
  });

  it("renders provider capability cards by stage from registry and readiness metadata", async () => {
    const checkedAt = Date.now();
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "asr.deepgram",
        status: "ready",
        message: "Deepgram key is valid",
        checked_at: checkedAt,
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "deepgram_api_key", present: true }],
        model_count: 2,
      },
      {
        provider_id: "asr.soniox",
        status: "ready",
        message: "Soniox key is valid but provider remains planned",
        checked_at: checkedAt,
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "soniox_api_key", present: true }],
        model_count: 1,
      },
      {
        provider_id: "asr.moonshine",
        status: "unchecked",
        message: "Local model files are present but runtime is not wired yet.",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [],
        model_count: 3,
        runtime: {
          status: "runtime_unavailable",
          message: "Moonshine native runtime adapter is not wired yet.",
          required_feature: "asr-moonshine",
          runtime_version: null,
          model_id: "moonshine-small-streaming-en",
        },
      },
      {
        provider_id: "llm.openrouter",
        status: "ready",
        message: "OpenRouter API key is valid",
        checked_at: checkedAt,
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openrouter_api_key", present: true }],
        model_count: 1,
      },
      {
        provider_id: "tts.deepgram_aura",
        status: "ready",
        message: "Deepgram Aura key is valid",
        checked_at: checkedAt,
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "deepgram_api_key", present: true }],
        model_count: 12,
        voice_catalog: auraVoiceCatalogFixture(),
      },
      {
        provider_id: "realtime_agent.gemini_live",
        status: "ready",
        message: "Gemini key is valid",
        checked_at: checkedAt,
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "gemini_api_key", present: true }],
        model_count: 1,
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "deepgram_api_key",
            present: true,
            source: "credentials_yaml",
          },
          {
            key: "openrouter_api_key",
            present: true,
            source: "credentials_yaml",
          },
          {
            key: "gemini_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    const capabilityOverview = await screen.findByRole("region", {
      name: /provider capability overview/i,
    });
    expect(
      within(capabilityOverview).getByRole("heading", {
        name: /ASR capabilities/i,
      }),
    ).toBeInTheDocument();
    expect(
      within(capabilityOverview).getByRole("heading", {
        name: /LLM capabilities/i,
      }),
    ).toBeInTheDocument();
    expect(
      within(capabilityOverview).getByRole("heading", {
        name: /TTS capabilities/i,
      }),
    ).toBeInTheDocument();
    expect(
      within(capabilityOverview).getByRole("heading", {
        name: /Realtime capabilities/i,
      }),
    ).toBeInTheDocument();

    const deepgramCard =
      await capabilityCardForProvider(/^Deepgram streaming$/i);
    expect(deepgramCard).toHaveTextContent(/asr\.deepgram/i);
    expect(deepgramCard).toHaveTextContent(/Stage\s*ASR/i);
    expect(deepgramCard).toHaveTextContent(/Selectable/i);
    expect(deepgramCard).toHaveTextContent(/Streaming\s*Yes/i);
    expect(deepgramCard).toHaveTextContent(/Partial revisions\s*Yes/i);
    expect(deepgramCard).toHaveTextContent(/Diarization\s*Yes/i);
    expect(deepgramCard).toHaveTextContent(/Pipeline audio\s*16 kHz mono f32/i);
    expect(deepgramCard).toHaveTextContent(
      /Provider audio\s*16 kHz mono PCM s16 LE/i,
    );
    expect(deepgramCard).toHaveTextContent(/Wire encoding\s*WebSocket binary/i);
    expect(deepgramCard).toHaveTextContent(
      /Resampling\s*No adapter resampling/i,
    );
    expect(deepgramCard).toHaveTextContent(/Multichannel\s*No/i);
    expect(deepgramCard).toHaveTextContent(
      /Events\s*Transcript partial\/final\/turn events/i,
    );
    expect(deepgramCard).toHaveTextContent(
      /Source policy\s*Mixed selected sources/i,
    );
    expect(deepgramCard).toHaveTextContent(/Auth\s*Saved key/i);
    expect(deepgramCard).toHaveTextContent(
      /Credential keys\s*deepgram_api_key/i,
    );
    expect(deepgramCard).toHaveTextContent(/Transport\s*WebSocket/i);
    expect(deepgramCard).toHaveTextContent(/Session\s*WebSocket/i);
    expect(deepgramCard).toHaveTextContent(/Keepalive\s*Control message/i);
    expect(deepgramCard).toHaveTextContent(/Close\s*End stream then close/i);
    expect(deepgramCard).toHaveTextContent(/Model catalog\s*Remote catalog/i);
    expect(deepgramCard).toHaveTextContent(/Default model\s*nova-3/i);
    expect(deepgramCard).toHaveTextContent(/Catalog count\s*2 models/i);
    expect(deepgramCard).toHaveTextContent(/Data boundary\s*Vendor cloud/i);
    expect(deepgramCard).toHaveTextContent(
      /Health probes\s*Command: test_deepgram_connection/i,
    );
    expect(deepgramCard).toHaveTextContent(
      /Platform blockers\s*None declared/i,
    );
    expect(deepgramCard).toHaveTextContent(/Readiness\s*Ready/i);
    expect(deepgramCard).toHaveTextContent(/Deepgram key is valid/i);

    const sonioxCard = await capabilityCardForProvider(/^Soniox realtime$/i);
    expect(sonioxCard).toHaveTextContent(/Planned/i);
    expect(sonioxCard).toHaveTextContent(/Streaming\s*Yes/i);
    expect(sonioxCard).toHaveTextContent(/Diarization\s*Yes/i);
    expect(sonioxCard).toHaveTextContent(/Readiness\s*Ready/i);
    expect(sonioxCard).toHaveTextContent(
      /planned providers are not selectable/i,
    );
    expect(
      within(sonioxCard).queryByRole("button", {
        name: /select soniox realtime/i,
      }),
    ).not.toBeInTheDocument();

    const moonshineCard = await capabilityCardForProvider(
      /^Moonshine local streaming$/i,
    );
    expect(moonshineCard).toHaveTextContent(
      /Runtime packaging\s*Feature: asr-moonshine/i,
    );
    expect(moonshineCard).toHaveTextContent(
      /Platform blockers\s*Planned provider gate, Feature: asr-moonshine/i,
    );
    expect(moonshineCard).toHaveTextContent(/Runtime unavailable/i);
    expect(moonshineCard).toHaveTextContent(/native runtime adapter/i);

    const googleCard = await capabilityCardForProvider(/^Google Chirp 3$/i);
    expect(googleCard).toHaveTextContent(/Planned/i);
    expect(googleCard).toHaveTextContent(/Transport\s*gRPC bidirectional/i);
    expect(googleCard).toHaveTextContent(/Wire encoding\s*gRPC streaming/i);
    expect(googleCard).toHaveTextContent(
      /Events\s*Transcript partial \+ final/i,
    );
    expect(googleCard).toHaveTextContent(
      /Endpoint modes\s*Default region, Custom endpoint/i,
    );
    expect(googleCard).toHaveTextContent(
      /Runtime packaging\s*Protobuf\/gRPC client/i,
    );
    expect(googleCard).toHaveTextContent(
      /Speaker labels\s*Streaming labels unverified/i,
    );
    expect(googleCard).toHaveTextContent(/local timeline recommended/i);
    expect(googleCard).toHaveTextContent(
      /Health probes\s*Token acquisition, Metadata only, Streaming RPC availability, Live env-gated smoke/i,
    );
    expect(googleCard).toHaveTextContent(
      /Platform blockers\s*Planned provider gate, Protobuf\/gRPC client/i,
    );
    expect(
      within(googleCard).queryByRole("button", {
        name: /select google chirp 3/i,
      }),
    ).not.toBeInTheDocument();

    const azureCard = await capabilityCardForProvider(/^Azure Speech$/i);
    expect(azureCard).toHaveTextContent(/Planned/i);
    expect(azureCard).toHaveTextContent(/Transport\s*Native SDK/i);
    expect(azureCard).toHaveTextContent(/Wire encoding\s*Native SDK/i);
    expect(azureCard).toHaveTextContent(
      /Endpoint modes\s*Default region, Custom endpoint, Private endpoint, Sovereign cloud/i,
    );
    expect(azureCard).toHaveTextContent(
      /Runtime packaging\s*Native SDK assets, Native framework assets, System libraries, System certificates, Visual C\+\+ redistributable/i,
    );
    expect(azureCard).toHaveTextContent(
      /Speaker labels\s*Streaming provider labels/i,
    );
    expect(azureCard).toHaveTextContent(
      /Health probes\s*Token acquisition, SDK dependency, Endpoint connectivity, Live env-gated smoke/i,
    );
    expect(azureCard).toHaveTextContent(
      /Platform blockers\s*Planned provider gate, Native SDK assets, Native framework assets, System libraries, System certificates, Visual C\+\+ redistributable/i,
    );
    expect(
      within(azureCard).queryByRole("button", {
        name: /select azure speech/i,
      }),
    ).not.toBeInTheDocument();

    const xaiCard = await capabilityCardForProvider(
      /^xAI Grok Speech to Text Streaming$/i,
    );
    expect(xaiCard).toHaveTextContent(/Watch candidate/i);
    expect(xaiCard).toHaveTextContent(
      /Credential keys\s*Credential schema not wired/i,
    );
    expect(xaiCard).toHaveTextContent(
      /Credential state\s*Auth required; credential schema not wired/i,
    );
    expect(xaiCard).toHaveTextContent(
      /Roadmap auth\s*Auth required; credential schema not wired/i,
    );
    expect(xaiCard).toHaveTextContent(
      /Platform blockers\s*Watch candidate provider gate, Credential schema not wired/i,
    );
    expect(xaiCard).toHaveTextContent(
      /Watch candidates are not selectable from Settings/i,
    );
    expect(xaiCard).not.toHaveTextContent(/No credential required/i);
    expect(
      within(xaiCard).queryByRole("button", {
        name: /select xai grok speech to text streaming/i,
      }),
    ).not.toBeInTheDocument();

    const nvidiaCard = await capabilityCardForProvider(
      /^NVIDIA\/Together Nemotron ASR$/i,
    );
    expect(nvidiaCard).toHaveTextContent(/Enterprise watch/i);
    expect(nvidiaCard).toHaveTextContent(
      /Credential state\s*Auth required; credential schema not wired/i,
    );
    expect(nvidiaCard).toHaveTextContent(
      /Health probes\s*Metadata only, Streaming RPC availability, Live env-gated smoke/i,
    );
    expect(nvidiaCard).toHaveTextContent(
      /Enterprise watch candidates are not selectable from Settings/i,
    );
    expect(nvidiaCard).not.toHaveTextContent(/No credential required/i);
    expect(
      within(nvidiaCard).queryByRole("button", {
        name: /select nvidia\/together nemotron asr/i,
      }),
    ).not.toBeInTheDocument();

    const openrouterCard = await capabilityCardForProvider(/^OpenRouter$/i);
    expect(openrouterCard).toHaveTextContent(/Stage\s*LLM/i);
    expect(openrouterCard).toHaveTextContent(/Auth\s*Saved key/i);
    expect(openrouterCard).toHaveTextContent(
      /Credential state\s*Saved: openrouter_api_key/i,
    );
    expect(openrouterCard).toHaveTextContent(/Transport\s*HTTP/i);
    expect(openrouterCard).toHaveTextContent(/Model catalog\s*Remote catalog/i);

    const auraCard = await capabilityCardForProvider(/^Deepgram Aura$/i);
    expect(auraCard).toHaveTextContent(/Stage\s*TTS/i);
    expect(auraCard).toHaveTextContent(/Streaming\s*Yes/i);
    expect(auraCard).toHaveTextContent(/Diarization\s*No/i);
    expect(auraCard).toHaveTextContent(/Model catalog\s*Fixed catalog/i);
    expect(auraCard).toHaveTextContent(/Catalog count\s*12 voices/i);

    const geminiCard = await capabilityCardForProvider(/^Gemini Live$/i);
    expect(geminiCard).toHaveTextContent(/Stage\s*Realtime/i);
    expect(geminiCard).toHaveTextContent(/Auth\s*Google auth/i);
    expect(geminiCard).toHaveTextContent(
      /Source policy\s*Mixed selected sources/i,
    );
    expect(geminiCard).toHaveTextContent(/Data boundary\s*Provider account/i);

    const openaiRealtimeAgentCard = await capabilityCardForProvider(
      /^OpenAI Realtime voice agent$/i,
    );
    expect(openaiRealtimeAgentCard).toHaveTextContent(/Planned/i);
    expect(openaiRealtimeAgentCard).toHaveTextContent(
      /Provider audio\s*24 kHz mono PCM s16 LE/i,
    );
    expect(openaiRealtimeAgentCard).toHaveTextContent(
      /Wire encoding\s*WebSocket JSON base64/i,
    );
    expect(openaiRealtimeAgentCard).toHaveTextContent(
      /Resampling\s*Adapter resamples/i,
    );
    expect(openaiRealtimeAgentCard).toHaveTextContent(
      /Events\s*Native realtime audio\/text/i,
    );
    expect(openaiRealtimeAgentCard).toHaveTextContent(
      /Default model\s*gpt-realtime-2/i,
    );
    expect(
      within(openaiRealtimeAgentCard).queryByRole("button", {
        name: /select openai realtime voice agent/i,
      }),
    ).not.toBeInTheDocument();
  });

  it("keeps existing advanced controls reachable after selecting a capability card", async () => {
    render(<SettingsPage />);

    const deepgramCard =
      await capabilityCardForProvider(/^Deepgram streaming$/i);
    fireEvent.click(
      within(deepgramCard).getByRole("button", {
        name: /select deepgram streaming/i,
      }),
    );

    expect(
      screen.getByRole("tab", { name: /speech-to-text/i }),
    ).toHaveAttribute("aria-selected", "true");
    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    const advanced = openSettingsAdvancedControls(asrGroup);
    expect(
      within(advanced).getByLabelText(/deepgram endpointing/i),
    ).toBeInTheDocument();
    expect(
      within(advanced).getByLabelText(/max speakers/i),
    ).toBeInTheDocument();
  });

  it("preserves Deepgram and diarization advanced values after capability-card navigation", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({
      saveSettings,
      modelStatus: {
        whisper: "Ready",
        llm: "Ready",
        sortformer: "Ready",
      },
    });

    render(<SettingsPage />);

    const deepgramCard =
      await capabilityCardForProvider(/^Deepgram streaming$/i);
    fireEvent.click(
      within(deepgramCard).getByRole("button", {
        name: /select deepgram streaming/i,
      }),
    );

    await waitFor(() =>
      expect(
        screen.getByRole("tab", { name: /speech-to-text/i }),
      ).toHaveAttribute("aria-selected", "true"),
    );

    const diarizationSection = settingsSectionForHeading(/^Diarization$/i);
    fireEvent.change(
      within(diarizationSection).getByLabelText(/diarization mode/i),
      { target: { value: "hybrid" } },
    );
    fireEvent.change(
      within(diarizationSection).getByLabelText(/speaker count/i),
      { target: { value: "fixed" } },
    );
    const diarizationAdvanced =
      openSettingsAdvancedControls(diarizationSection);
    fireEvent.change(
      within(diarizationAdvanced).getByLabelText(/maximum speakers/i),
      { target: { value: "4" } },
    );

    const asrGroup = settingsSectionForHeading(/ASR Provider/i);
    const deepgramAdvanced = openSettingsAdvancedControls(asrGroup);
    fireEvent.change(
      within(deepgramAdvanced).getByLabelText(/deepgram endpointing/i),
      { target: { value: "425" } },
    );
    fireEvent.change(
      within(deepgramAdvanced).getByLabelText(/utteranceend gap/i),
      { target: { value: "1250" } },
    );
    fireEvent.click(
      within(deepgramAdvanced).getByLabelText(/vad turn-start events/i),
    );
    fireEvent.change(
      within(deepgramAdvanced).getByLabelText(/^flux eot threshold/i),
      { target: { value: "0.72" } },
    );
    fireEvent.change(
      within(deepgramAdvanced).getByLabelText(/eager eot threshold/i),
      { target: { value: "0.33" } },
    );
    fireEvent.change(within(deepgramAdvanced).getByLabelText(/eot timeout/i), {
      target: { value: "1800" },
    });
    fireEvent.change(within(deepgramAdvanced).getByLabelText(/max speakers/i), {
      target: { value: "6" },
    });

    goToTab(/general/i);
    goToTab(/speech-to-text/i);

    const asrGroupAfterRoundTrip = settingsSectionForHeading(/ASR Provider/i);
    expect(
      within(asrGroupAfterRoundTrip).getByRole("radio", {
        name: /^deepgram streaming$/i,
      }),
    ).toBeChecked();
    const deepgramAdvancedAfterRoundTrip = openSettingsAdvancedControls(
      asrGroupAfterRoundTrip,
    );
    expect(
      within(deepgramAdvancedAfterRoundTrip).getByLabelText(
        /deepgram endpointing/i,
      ),
    ).toHaveValue(425);
    expect(
      within(deepgramAdvancedAfterRoundTrip).getByLabelText(
        /utteranceend gap/i,
      ),
    ).toHaveValue(1250);
    expect(
      within(deepgramAdvancedAfterRoundTrip).getByLabelText(
        /vad turn-start events/i,
      ),
    ).not.toBeChecked();
    expect(
      within(deepgramAdvancedAfterRoundTrip).getByLabelText(
        /^flux eot threshold/i,
      ),
    ).toHaveValue(0.72);
    expect(
      within(deepgramAdvancedAfterRoundTrip).getByLabelText(
        /eager eot threshold/i,
      ),
    ).toHaveValue(0.33);
    expect(
      within(deepgramAdvancedAfterRoundTrip).getByLabelText(/eot timeout/i),
    ).toHaveValue(1800);
    expect(
      within(deepgramAdvancedAfterRoundTrip).getByLabelText(/max speakers/i),
    ).toHaveValue(6);

    const diarizationAfterRoundTrip =
      settingsSectionForHeading(/^Diarization$/i);
    expect(
      within(diarizationAfterRoundTrip).getByLabelText(/diarization mode/i),
    ).toHaveValue("hybrid");
    expect(
      within(diarizationAfterRoundTrip).getByLabelText(/speaker count/i),
    ).toHaveValue("fixed");
    const diarizationAdvancedAfterRoundTrip = openSettingsAdvancedControls(
      diarizationAfterRoundTrip,
    );
    expect(
      within(diarizationAdvancedAfterRoundTrip).getByLabelText(
        /maximum speakers/i,
      ),
    ).toHaveValue(4);

    await clickSaveSettings();

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0]?.[0];
    expect(saved.asr_provider.type).toBe("deepgram");
    if (saved.asr_provider.type === "deepgram") {
      expect(saved.asr_provider).toMatchObject({
        endpointing_ms: 425,
        utterance_end_ms: 1250,
        vad_events: false,
        eot_threshold: 0.72,
        eager_eot_threshold: 0.33,
        eot_timeout_ms: 1800,
        max_speakers: 6,
        enable_diarization: true,
      });
    }
    expect(saved.diarization).toEqual({
      mode: "hybrid",
      speaker_count: "fixed",
      max_speakers: 4,
    });
  });

  it("preserves AWS Transcribe profile mode selected from a capability card", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    const listAwsProfiles = vi.fn(async () => ["dictation-prod"]);
    resetStore({ saveSettings, listAwsProfiles });

    render(<SettingsPage />);

    const awsCard = await capabilityCardForProvider(
      /^AWS Transcribe streaming$/i,
    );
    fireEvent.click(
      within(awsCard).getByRole("button", {
        name: /select aws transcribe streaming/i,
      }),
    );

    await waitFor(() =>
      expect(document.getElementById("aws-asr-region")).toHaveFocus(),
    );
    const asrGroup = settingsSectionForHeading(/ASR Provider/i);
    const advanced = openSettingsAdvancedControls(asrGroup);
    fireEvent.change(within(advanced).getByLabelText(/credential mode/i), {
      target: { value: "profile" },
    });

    await waitFor(() => expect(listAwsProfiles).toHaveBeenCalled());
    const profileSelect =
      await within(advanced).findByLabelText(/aws profile/i);
    fireEvent.change(profileSelect, {
      target: { value: "dictation-prod" },
    });

    goToTab(/general/i);
    goToTab(/speech-to-text/i);

    const roundTripAdvanced = openSettingsAdvancedControls(
      settingsSectionForHeading(/ASR Provider/i),
    );
    expect(
      within(roundTripAdvanced).getByLabelText(/credential mode/i),
    ).toHaveValue("profile");
    expect(
      within(roundTripAdvanced).getByLabelText(/aws profile/i),
    ).toHaveValue("dictation-prod");

    await clickSaveSettings();

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0]?.[0];
    expect(saved.asr_provider).toMatchObject({
      type: "aws_transcribe",
      credential_source: { type: "profile", name: "dictation-prod" },
    });
    expect(saved.llm_provider.type).toBe("api");
  });

  it("preserves AWS Transcribe access-key mode from a capability card and saves credentials", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });

    render(<SettingsPage />);

    const awsCard = await capabilityCardForProvider(
      /^AWS Transcribe streaming$/i,
    );
    fireEvent.click(
      within(awsCard).getByRole("button", {
        name: /select aws transcribe streaming/i,
      }),
    );

    await waitFor(() =>
      expect(document.getElementById("aws-asr-region")).toHaveFocus(),
    );
    const asrGroup = settingsSectionForHeading(/ASR Provider/i);
    const advanced = openSettingsAdvancedControls(asrGroup);
    fireEvent.change(within(advanced).getByLabelText(/credential mode/i), {
      target: { value: "access_keys" },
    });
    const accessKeyInput = await openCredentialInput(
      advanced,
      /access key id/i,
      /add aws keys|replace aws keys/i,
    );
    fireEvent.change(accessKeyInput, {
      target: { value: "AKIA_CARD" },
    });
    fireEvent.change(
      within(advanced).getByLabelText(/secret access key/i, {
        selector: "input",
      }),
      { target: { value: "secret-card" } },
    );
    fireEvent.change(
      within(advanced).getByLabelText(/session token/i, {
        selector: "input",
      }),
      { target: { value: "session-card" } },
    );

    goToTab(/general/i);
    goToTab(/speech-to-text/i);

    const roundTripAdvanced = openSettingsAdvancedControls(
      settingsSectionForHeading(/ASR Provider/i),
    );
    expect(
      within(roundTripAdvanced).getByLabelText(/credential mode/i),
    ).toHaveValue("access_keys");
    expect(
      within(roundTripAdvanced).getByLabelText(/access key id/i, {
        selector: "input",
      }),
    ).toHaveValue("AKIA_CARD");
    expect(
      within(roundTripAdvanced).getByLabelText(/secret access key/i, {
        selector: "input",
      }),
    ).toHaveValue("secret-card");
    expect(
      within(roundTripAdvanced).getByLabelText(/session token/i, {
        selector: "input",
      }),
    ).toHaveValue("session-card");

    await clickSaveSettings();

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0]?.[0];
    expect(saved.asr_provider).toMatchObject({
      type: "aws_transcribe",
      credential_source: { type: "access_keys", access_key: "" },
    });
    expect(saveCredentialCalls()).toEqual(
      expect.arrayContaining([
        ["save_credential_cmd", { key: "aws_access_key", value: "AKIA_CARD" }],
        [
          "save_credential_cmd",
          { key: "aws_secret_key", value: "secret-card" },
        ],
        [
          "save_credential_cmd",
          { key: "aws_session_token", value: "session-card" },
        ],
      ]),
    );
    const saveSettingsOrder = saveSettings.mock.invocationCallOrder[0];
    const awsSecretOrder = mockedInvoke.mock.calls.findIndex(
      ([cmd, args]) =>
        cmd === "save_credential_cmd" &&
        (args as { key?: string } | undefined)?.key === "aws_secret_key",
    );
    const awsSessionOrder = mockedInvoke.mock.calls.findIndex(
      ([cmd, args]) =>
        cmd === "save_credential_cmd" &&
        (args as { key?: string } | undefined)?.key === "aws_session_token",
    );
    expect(mockedInvoke.mock.invocationCallOrder[awsSecretOrder]).toBeLessThan(
      saveSettingsOrder,
    );
    expect(mockedInvoke.mock.invocationCallOrder[awsSessionOrder]).toBeLessThan(
      saveSettingsOrder,
    );
  });

  it("preserves OpenRouter advanced options after capability-card navigation", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });

    render(<SettingsPage />);

    const openrouterCard = await capabilityCardForProvider(/^OpenRouter$/i);
    fireEvent.click(
      within(openrouterCard).getByRole("button", {
        name: /select openrouter/i,
      }),
    );

    await waitFor(() =>
      expect(document.getElementById("llm-openrouter-model")).toHaveFocus(),
    );
    const llmGroup = settingsSectionForHeading(/LLM Provider/i);
    fireEvent.change(
      await openCredentialInput(llmGroup, /openrouter api key/i),
      {
        target: { value: "sk-or-card" },
      },
    );
    fireEvent.change(
      within(llmGroup).getByRole("combobox", { name: /openrouter model/i }),
      { target: { value: "anthropic/claude-3.5-haiku" } },
    );
    const advanced = openSettingsAdvancedControls(llmGroup);
    fireEvent.change(within(advanced).getByLabelText(/endpoint/i), {
      target: { value: "https://openrouter.example/api/v1/" },
    });
    fireEvent.click(
      within(advanced).getByRole("checkbox", {
        name: /include token usage/i,
      }),
    );

    goToTab(/general/i);
    goToTab(/language model/i);

    const llmGroupAfterRoundTrip = settingsSectionForHeading(/LLM Provider/i);
    expect(
      within(llmGroupAfterRoundTrip).getByRole("radio", {
        name: /openrouter/i,
      }),
    ).toBeChecked();
    expect(
      within(llmGroupAfterRoundTrip).getByRole("combobox", {
        name: /openrouter model/i,
      }),
    ).toHaveValue("anthropic/claude-3.5-haiku");
    const advancedAfterRoundTrip = openSettingsAdvancedControls(
      llmGroupAfterRoundTrip,
    );
    expect(
      within(advancedAfterRoundTrip).getByLabelText(/endpoint/i),
    ).toHaveValue("https://openrouter.example/api/v1/");
    expect(
      within(advancedAfterRoundTrip).getByRole("checkbox", {
        name: /include token usage/i,
      }),
    ).not.toBeChecked();

    await clickSaveSettings();

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0]?.[0];
    expect(saved.llm_provider.type).toBe("openrouter");
    if (saved.llm_provider.type === "openrouter") {
      expect(saved.llm_provider).toMatchObject({
        model: "anthropic/claude-3.5-haiku",
        base_url: "https://openrouter.example/api/v1",
        include_usage_in_stream: false,
      });
    }
    expect(mockedInvoke).toHaveBeenCalledWith("save_credential_cmd", {
      key: "openrouter_api_key",
      value: "sk-or-card",
    });
  });

  it("preserves Gemini Vertex fields after opening Gemini Live from its capability card", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });

    render(<SettingsPage />);

    const geminiCard = await capabilityCardForProvider(/^Gemini Live$/i);
    fireEvent.click(
      within(geminiCard).getByRole("button", {
        name: /select gemini live/i,
      }),
    );

    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /gemini/i })).toHaveAttribute(
        "aria-selected",
        "true",
      ),
    );
    const geminiSection = settingsSectionForHeading(/Gemini Live/i);
    fireEvent.click(
      within(geminiSection).getByRole("radio", { name: /vertex ai/i }),
    );
    fireEvent.change(within(geminiSection).getByLabelText(/project id/i), {
      target: { value: "audio-prod" },
    });
    fireEvent.change(within(geminiSection).getByLabelText(/location/i), {
      target: { value: "us-central1" },
    });
    fireEvent.change(
      await openCredentialInput(geminiSection, /service account path/i),
      { target: { value: "/secure/audio-prod-sa.json" } },
    );
    fireEvent.change(
      within(geminiSection).getByRole("combobox", { name: /^model$/i }),
      { target: { value: "gemini-2.0-flash-live-001" } },
    );

    goToTab(/general/i);
    goToTab(/gemini/i);

    const geminiAfterRoundTrip = settingsSectionForHeading(/Gemini Live/i);
    expect(
      within(geminiAfterRoundTrip).getByRole("radio", {
        name: /vertex ai/i,
      }),
    ).toBeChecked();
    expect(
      within(geminiAfterRoundTrip).getByLabelText(/project id/i),
    ).toHaveValue("audio-prod");
    expect(
      within(geminiAfterRoundTrip).getByLabelText(/location/i),
    ).toHaveValue("us-central1");
    expect(
      within(geminiAfterRoundTrip).getByLabelText(/service account path/i, {
        selector: "input",
      }),
    ).toHaveValue("/secure/audio-prod-sa.json");

    await clickSaveSettings();

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0]?.[0];
    expect(saved.gemini).toEqual({
      auth: {
        type: "vertex_ai",
        project_id: "audio-prod",
        location: "us-central1",
        service_account_path: "/secure/audio-prod-sa.json",
      },
      model: "gemini-2.0-flash-live-001",
    });
    expect(mockedInvoke).toHaveBeenCalledWith("save_credential_cmd", {
      key: "google_service_account_path",
      value: "/secure/audio-prod-sa.json",
    });
  });

  it("preserves Deepgram Aura TTS tuning after capability-card navigation", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });

    render(<SettingsPage />);

    const auraCard = await capabilityCardForProvider(/^Deepgram Aura$/i);
    fireEvent.click(
      within(auraCard).getByRole("button", {
        name: /select deepgram aura/i,
      }),
    );

    await waitFor(() =>
      expect(document.getElementById("tts-provider-select")).toHaveFocus(),
    );
    const ttsSection = settingsSectionForHeading(/Text-to-Speech/i);
    fireEvent.change(
      within(ttsSection).getByRole("combobox", { name: /^voice$/i }),
      { target: { value: "aura-luna-en" } },
    );
    fireEvent.change(within(ttsSection).getByLabelText(/speed/i), {
      target: { value: "1.2" },
    });
    fireEvent.click(within(ttsSection).getByLabelText(/speak chatbot/i));
    fireEvent.change(
      await openCredentialInput(ttsSection, /deepgram api key/i),
      {
        target: { value: "dg-tts-card" },
      },
    );

    goToTab(/general/i);
    goToTab(/text-to-speech/i);

    const ttsAfterRoundTrip = settingsSectionForHeading(/Text-to-Speech/i);
    expect(within(ttsAfterRoundTrip).getByLabelText(/^provider$/i)).toHaveValue(
      "deepgram_aura",
    );
    expect(
      within(ttsAfterRoundTrip).getByRole("combobox", { name: /^voice$/i }),
    ).toHaveValue("aura-luna-en");
    expect(within(ttsAfterRoundTrip).getByLabelText(/speed/i)).toHaveValue(1.2);
    expect(
      within(ttsAfterRoundTrip).getByLabelText(/speak chatbot/i),
    ).toBeChecked();

    await clickSaveSettings();

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0]?.[0];
    expect(saved.tts_provider).toEqual({
      type: "deepgram_aura",
      voice: "aura-luna-en",
      sample_rate: 24000,
      speed: 1.2,
    });
    expect(saved.speak_aloud).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith("save_credential_cmd", {
      key: "deepgram_api_key",
      value: "dg-tts-card",
    });
  });

  it("renders ready local-private product mode details and focuses model controls", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        asr_provider: { type: "local_whisper" },
        llm_provider: { type: "local_llama" },
      },
    });
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") return [];
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "asr.local_whisper",
            status: "ready",
            message: "Local Whisper model ready",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [],
          },
          {
            provider_id: "llm.local_llama",
            status: "ready",
            message: "Local llama.cpp model ready",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [],
          },
        ];
      }
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    const localCard = await modeOverviewCard(/local private/i);
    expect(within(localCard).getByText("Selected")).toBeInTheDocument();
    expect(within(localCard).getByText("Ready")).toBeInTheDocument();
    expect(within(localCard).getByText("Local only")).toBeInTheDocument();
    expect(
      within(localCard).getAllByText(/Speech-to-text/i).length,
    ).toBeGreaterThan(0);
    expect(within(localCard).getByText(/^Local Whisper$/i)).toBeInTheDocument();
    expect(
      within(localCard).getByText("ggml-small.en.bin"),
    ).toBeInTheDocument();
    expect(
      within(localCard).getAllByText(/Notes and graph/i).length,
    ).toBeGreaterThan(0);
    expect(
      within(localCard).getByText(/^Local llama\.cpp$/i),
    ).toBeInTheDocument();
    expect(within(localCard).getByText("llama3.2")).toBeInTheDocument();
    expect(within(localCard).getByText(/No blockers/i)).toBeInTheDocument();

    fireEvent.click(
      within(localCard).getByRole("button", {
        name: /choose local private model/i,
      }),
    );

    await waitFor(() =>
      expect(document.getElementById("asr-whisper-model")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /speech-to-text/i }),
    ).toHaveAttribute("aria-selected", "true");
  });

  it("does not mark Gemini Live active in notes mode from a stale legacy native flag", async () => {
    resetStore({
      nativeS2sEnabled: true,
      conversationMode: "notes",
      converseEngine: "native",
      settings: {
        ...baseSettings,
        asr_provider: { type: "local_whisper" },
        llm_provider: { type: "local_llama" },
      },
    });
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "gemini_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "asr.local_whisper",
            status: "ready",
            message: "Local Whisper model ready",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [],
          },
          {
            provider_id: "llm.local_llama",
            status: "ready",
            message: "Local llama.cpp model ready",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [],
          },
          {
            provider_id: "realtime_agent.gemini_live",
            status: "ready",
            message: "Gemini API key is valid",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "gemini_api_key", present: true }],
          },
        ];
      }
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    const localCard = await modeOverviewCard(/local private/i);
    expect(within(localCard).getByText("Selected")).toBeInTheDocument();
    const nativeCard = await modeOverviewCard(/native realtime/i);
    expect(within(nativeCard).queryByText("Selected")).not.toBeInTheDocument();
    const geminiCard = await capabilityCardForProvider(/^Gemini Live$/i);
    expect(within(geminiCard).queryByText("Selected")).not.toBeInTheDocument();
    expect(
      screen.queryByText("Selected: gemini-3.1-flash-live-preview"),
    ).not.toBeInTheDocument();
    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("get_provider_readiness_cmd", {
        refresh: true,
        conversationMode: "notes",
        converseEngine: "native",
        requestId: expect.any(String),
      }),
    );
  });

  it("marks Gemini Live active when runtime mode is converse native", async () => {
    resetStore({
      nativeS2sEnabled: true,
      conversationMode: "converse",
      converseEngine: "native",
      settings: {
        ...baseSettings,
        asr_provider: { type: "local_whisper" },
        llm_provider: { type: "local_llama" },
      },
    });
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "gemini_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "asr.local_whisper",
            status: "ready",
            message: "Local Whisper model ready",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [],
          },
          {
            provider_id: "llm.local_llama",
            status: "ready",
            message: "Local llama.cpp model ready",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [],
          },
          {
            provider_id: "realtime_agent.gemini_live",
            status: "ready",
            message: "Gemini API key is valid",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "gemini_api_key", present: true }],
          },
        ];
      }
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    const nativeCard = await modeOverviewCard(/native realtime/i);
    expect(within(nativeCard).getByText("Selected")).toBeInTheDocument();
    expect(within(nativeCard).getByText(/^Gemini Live$/i)).toBeInTheDocument();
    const geminiCard = await capabilityCardForProvider(/^Gemini Live$/i);
    expect(within(geminiCard).getByText("Selected")).toBeInTheDocument();
    expect(
      await screen.findByText("Selected: gemini-3.1-flash-live-preview"),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("get_provider_readiness_cmd", {
        refresh: true,
        conversationMode: "converse",
        converseEngine: "native",
        requestId: expect.any(String),
      }),
    );
  });

  it("renders source blockers in product mode cards without platform-specific copy", async () => {
    const closeSettings = vi.fn();
    resetStore({
      selectedSourceIds: [],
      closeSettings,
      settings: {
        ...baseSettings,
        asr_provider: { type: "local_whisper" },
        llm_provider: { type: "local_llama" },
      },
    });
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") return [];
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "asr.local_whisper",
            status: "ready",
            message: "Local Whisper model ready",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [],
          },
          {
            provider_id: "llm.local_llama",
            status: "ready",
            message: "Local llama.cpp model ready",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [],
          },
        ];
      }
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    const localCard = await modeOverviewCard(/local private/i);
    await waitFor(() =>
      expect(within(localCard).getByText("Blocked")).toBeInTheDocument(),
    );
    expect(within(localCard).getByText("Source:")).toBeInTheDocument();
    expect(
      within(localCard).getByText(
        /Local Whisper needs an audio source selection/i,
      ),
    ).toBeInTheDocument();
    const sourcesAction = within(localCard).getByRole("button", {
      name: /review local private source selection/i,
    });
    fireEvent.click(sourcesAction);
    expect(closeSettings).toHaveBeenCalled();
    expect(useAudioGraphStore.getState().sourceRecoveryIntent).toMatchObject({
      origin: "provider_setup",
      issues: [
        expect.objectContaining({
          kind: "unselected",
          message: expect.stringMatching(
            /Local Whisper needs an audio source selection/i,
          ),
        }),
      ],
    });
  });

  it("renders hybrid partial mode and routes cloud credential blockers", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        asr_provider: { type: "local_whisper" },
        llm_provider: {
          type: "openrouter",
          model: "anthropic/claude-3.5-haiku",
          base_url: "https://openrouter.ai/api/v1",
          provider_order: null,
          include_usage_in_stream: true,
          api_key: "",
        },
      },
    });
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openrouter_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "asr.local_whisper",
            status: "ready",
            message: "Local Whisper model ready",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [],
          },
          {
            provider_id: "llm.openrouter",
            status: "ready",
            message: "OpenRouter key is valid",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "openrouter_api_key", present: true }],
          },
        ];
      }
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    const hybridCard = await modeOverviewCard(/hybrid/i);
    await waitFor(() =>
      expect(within(hybridCard).getByText(/^OpenRouter$/i)).toBeInTheDocument(),
    );
    expect(within(hybridCard).getByText("Selected")).toBeInTheDocument();
    expect(within(hybridCard).getByText("Ready")).toBeInTheDocument();
    expect(
      within(hybridCard).getByText(/Mixed local and cloud/i),
    ).toBeInTheDocument();
    expect(
      within(hybridCard).getByText("anthropic/claude-3.5-haiku"),
    ).toBeInTheDocument();

    const cloudCard = await modeOverviewCard(/cloud fast/i);
    await waitFor(() =>
      expect(
        within(cloudCard).getByText(/deepgram_api_key/),
      ).toBeInTheDocument(),
    );
    expect(within(cloudCard).getByText("Missing key")).toBeInTheDocument();
    expect(
      within(cloudCard).getByText(/^Deepgram streaming$/i),
    ).toBeInTheDocument();

    fireEvent.click(
      within(cloudCard).getByRole("button", {
        name: /fix cloud fast credential/i,
      }),
    );

    await waitFor(() =>
      expect(document.getElementById("deepgram-api-key")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /speech-to-text/i }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("radio", { name: /^deepgram streaming$/i }),
    ).toBeChecked();
  });

  it("renders missing cloud mode credential and model blockers with model routing", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        asr_provider: {
          type: "deepgram",
          api_key: "",
          model: "nova-3",
          enable_diarization: true,
        },
        llm_provider: {
          type: "openrouter",
          model: "",
          base_url: "https://openrouter.ai/api/v1",
          provider_order: null,
          include_usage_in_stream: true,
          api_key: "",
        },
      },
    });

    render(<SettingsPage />);

    const cloudCard = await modeOverviewCard(/cloud fast/i);
    await waitFor(() =>
      expect(within(cloudCard).getByText("Selected")).toBeInTheDocument(),
    );
    expect(within(cloudCard).getByText("Missing key")).toBeInTheDocument();
    expect(within(cloudCard).getByText(/deepgram_api_key/)).toBeInTheDocument();
    expect(
      within(cloudCard).getByText(/openrouter_api_key/),
    ).toBeInTheDocument();
    expect(
      within(cloudCard).getByText(/OpenRouter needs a selected model/i),
    ).toBeInTheDocument();

    fireEvent.click(
      within(cloudCard).getByRole("button", {
        name: /choose cloud fast model/i,
      }),
    );

    await waitFor(() =>
      expect(document.getElementById("llm-openrouter-model")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /language model/i }),
    ).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("radio", { name: /openrouter/i })).toBeChecked();
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("checks provider readiness on open without loading plaintext credentials", async () => {
    render(<SettingsPage />);

    const status = screen.getByRole("status", {
      name: /provider readiness/i,
    });
    expect(status).toHaveClass("sr-only");
    expect(status).toHaveAttribute("aria-live", "polite");
    expect(status).toHaveAttribute("aria-atomic", "true");
    expect(status).toHaveAttribute("aria-busy");
    expect(status).toHaveTextContent(/provider readiness/i);

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("get_provider_readiness_cmd", {
        ...notesReadinessArgs(),
      }),
    );
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("announces blocked and stale provider readiness updates in the hidden status summary", async () => {
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
      if (cmd === "load_credential_presence_cmd") return [];
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "asr.deepgram",
            status: "blocked",
            message: "Deepgram blocked by policy",
            checked_at: null,
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "deepgram_api_key", present: false }],
          },
          {
            provider_id: "llm.openrouter",
            status: "unchecked",
            message: "OpenRouter policy checks are blocked",
            checked_at: null,
            stale: true,
            credential_epoch: 0,
            credentials: [],
          },
        ] as unknown as ProviderReadiness[];
      }
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    const row = await readinessRowForProvider(/^Deepgram streaming$/i);
    const liveStatus = screen.getByRole("status", {
      name: /provider readiness/i,
    });
    expect(liveStatus).toHaveTextContent(/provider readiness/i);
    expect(liveStatus).toHaveTextContent(/blocked 1/i);
    expect(liveStatus).toHaveTextContent(/unchecked 1/i);
    expect(liveStatus).toHaveTextContent(/cached result may be stale/i);
    expect(liveStatus).not.toContainElement(row);

    expect(row.closest('[role="status"]')).toBeNull();
    const openCredential = within(row).getByRole("button", {
      name: /add or replace credential/i,
    });
    expect(openCredential.closest('[role="status"]')).toBeNull();

    const visibleStatus = document.querySelector(".settings-readiness__status");
    expect(visibleStatus).toBeInstanceOf(HTMLElement);
    expect(visibleStatus).not.toHaveAttribute("role");
    expect(visibleStatus).not.toHaveAttribute("aria-live");
    expect(visibleStatus).not.toHaveAttribute("aria-atomic");
    expect(visibleStatus).not.toHaveAttribute("aria-busy");
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("keeps provider readiness list details and actions outside the live status region", async () => {
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "deepgram_api_key",
            present: false,
            source: "missing",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "asr.deepgram",
            status: "missing_credentials",
            message: "Missing saved credential(s): deepgram_api_key",
            checked_at: null,
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "deepgram_api_key", present: false }],
          },
        ] satisfies ProviderReadiness[];
      }
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    const row = await readinessRowForProvider(/^Deepgram streaming$/i);
    const liveStatus = screen.getByRole("status", {
      name: /provider readiness/i,
    });
    expect(liveStatus).toHaveClass("sr-only");
    expect(liveStatus).toHaveAttribute("aria-live", "polite");
    expect(liveStatus).toHaveAttribute("aria-atomic", "true");
    expect(liveStatus).toHaveTextContent(/provider readiness/i);
    expect(liveStatus).toHaveTextContent(/missing key 1/i);
    expect(liveStatus).not.toContainElement(row);

    const visibleStatus = document.querySelector(".settings-readiness__status");
    expect(visibleStatus).toBeInstanceOf(HTMLElement);
    expect(visibleStatus).not.toHaveAttribute("role");
    expect(visibleStatus).not.toHaveAttribute("aria-live");
    expect(visibleStatus).not.toHaveAttribute("aria-atomic");
    expect(visibleStatus).not.toHaveAttribute("aria-busy");

    expect(row.closest('[role="status"]')).toBeNull();
    const recovery = within(row).getByText(
      /add the missing key in this provider section/i,
    );
    expect(recovery.closest('[role="status"]')).toBeNull();
    const detailsSummary = within(row).getByText(/details/i, {
      selector: "summary",
    });
    expect(detailsSummary.closest('[role="status"]')).toBeNull();
    const openCredential = within(row).getByRole("button", {
      name: /add or replace credential/i,
    });
    expect(openCredential.closest('[role="status"]')).toBeNull();
  });

  it("cancels an in-flight provider readiness request when Settings unmounts", async () => {
    let resolveReadiness: (readiness: ProviderReadiness[]) => void = () => {};
    mockedInvoke.mockImplementation((cmd: string) => {
      if (cmd === "load_credential_presence_cmd") return Promise.resolve([]);
      if (cmd === "get_provider_readiness_cmd") {
        return new Promise<ProviderReadiness[]>((resolve) => {
          resolveReadiness = resolve;
        });
      }
      if (cmd === "cancel_provider_readiness_cmd") return Promise.resolve(true);
      if (cmd === "list_aws_profiles") return Promise.resolve([]);
      return Promise.resolve(undefined);
    });

    const { unmount } = render(<SettingsPage />);

    await waitFor(() => expect(providerReadinessCalls()).toHaveLength(1));
    const requestId = (
      providerReadinessCalls()[0]?.[1] as { requestId: string } | undefined
    )?.requestId;
    expect(requestId).toEqual(expect.any(String));

    unmount();

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith(
        "cancel_provider_readiness_cmd",
        { requestId },
      ),
    );
    await act(async () => {
      resolveReadiness([]);
    });
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("surfaces credential-file errors in provider readiness instead of showing empty state", async () => {
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (
        cmd === "load_credential_presence_cmd" ||
        cmd === "get_provider_readiness_cmd"
      ) {
        throw {
          code: "credential_file_error",
          message: { reason: "malformed credentials.yaml" },
        };
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    expect(
      await screen.findByText(/malformed credentials\.yaml/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/repair the local credential store/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/no saved provider credentials/i),
    ).not.toBeInTheDocument();
  });

  it("lets users manually rerun saved-credential readiness checks", async () => {
    render(<SettingsPage />);

    await waitFor(() => expect(providerReadinessCalls()).toHaveLength(1));
    const initialRequestId = (
      providerReadinessCalls()[0]?.[1] as { requestId?: string } | undefined
    )?.requestId;
    const runChecks = screen.getByRole("button", { name: /run checks/i });
    await waitFor(() => expect(runChecks).not.toBeDisabled());

    await act(async () => {
      fireEvent.click(runChecks);
    });

    await waitFor(() =>
      expect(providerReadinessCalls().length).toBeGreaterThanOrEqual(2),
    );
    expect(providerReadinessCalls().at(-1)).toEqual([
      "get_provider_readiness_cmd",
      notesReadinessArgs(true),
    ]);
    const refreshRequestId = (
      providerReadinessCalls().at(-1)?.[1] as { requestId?: string } | undefined
    )?.requestId;
    expect(refreshRequestId).toEqual(expect.any(String));
    expect(refreshRequestId).not.toBe(initialRequestId);
  });

  it("renders credential health rows with replace, retest, and clear actions without plaintext loadback", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    resetStore({
      settings: {
        ...baseSettings,
        llm_provider: {
          type: "openrouter",
          model: "anthropic/claude-sonnet-4.5",
          base_url: "https://openrouter.ai/api/v1",
          provider_order: null,
          include_usage_in_stream: true,
          api_key: "",
        },
      },
    });
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "llm.openrouter",
        status: "ready",
        message: "OpenRouter API key is valid",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openrouter_api_key", present: true }],
      },
      {
        provider_id: "asr.deepgram",
        status: "error",
        message: "Deepgram key failed validation",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "deepgram_api_key", present: true }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openrouter_api_key",
            present: true,
            source: "credentials_yaml",
          },
          {
            key: "deepgram_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback(args);
      if (cmd === "delete_credential_cmd") return undefined;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    expect(await screen.findByText(/credential health/i)).toBeInTheDocument();
    const openrouterRow = screen
      .getAllByText("openrouter_api_key")
      .map((node) => node.closest(".settings-credential-health__item"))
      .find((node): node is HTMLElement => node instanceof HTMLElement);
    if (!openrouterRow) throw new Error("openrouter credential row not found");
    expect(
      within(openrouterRow).getByText(/credentials\.yaml/i),
    ).toBeInTheDocument();
    expect(
      within(openrouterRow).getByText(/OpenRouter: Ready/i),
    ).toBeInTheDocument();
    expect(screen.queryByText("sk-or-should-not-load")).not.toBeInTheDocument();

    fireEvent.click(
      within(openrouterRow).getByRole("button", { name: /replace/i }),
    );
    await waitFor(() =>
      expect(document.getElementById("llm-openrouter-api-key")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /language model/i }),
    ).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("radio", { name: /openrouter/i })).toBeChecked();

    await act(async () => {
      fireEvent.click(
        within(openrouterRow).getByRole("button", { name: /retest/i }),
      );
    });
    await waitFor(() =>
      expect(providerReadinessCalls().length).toBeGreaterThanOrEqual(2),
    );
    expect(providerReadinessCalls().at(-1)).toEqual([
      "get_provider_readiness_cmd",
      notesReadinessArgs(true),
    ]);

    await act(async () => {
      fireEvent.click(
        within(openrouterRow).getByRole("button", { name: /clear/i }),
      );
    });
    expect(mockedInvoke).toHaveBeenCalledWith("delete_credential_cmd", {
      key: "openrouter_api_key",
    });
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
    confirmSpy.mockRestore();
  });

  it("routes shared openai_api_key replacement to the active LLM API field before active ASR", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        asr_provider: {
          type: "openai_realtime",
          api_key: "",
          model: "gpt-realtime-whisper",
          language: null,
        },
        llm_provider: {
          type: "api",
          endpoint: "https://api.openai.com/v1",
          api_key: "",
          model: "gpt-4o-mini",
        },
      },
    });
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "asr.openai_realtime",
        status: "ready",
        message: "OpenAI Realtime key is valid",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openai_api_key", present: true }],
      },
      {
        provider_id: "llm.api",
        status: "ready",
        message: "OpenAI-compatible LLM key is valid",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openai_api_key", present: true }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openai_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    await waitFor(() =>
      expect(credentialHealthRowForKey("openai_api_key")).toBeInTheDocument(),
    );
    const row = credentialHealthRowForKey("openai_api_key");
    expect(
      within(row).getByText(/OpenAI-compatible LLM: Ready/i),
    ).toBeInTheDocument();

    fireEvent.click(within(row).getByRole("button", { name: /replace/i }));

    await waitFor(() =>
      expect(document.getElementById("llm-custom-api-key")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /language model/i }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByText(/saved key available for this endpoint/i),
    ).toBeInTheDocument();
    const llmApiKeyInput = document.getElementById(
      "llm-custom-api-key",
    ) as HTMLInputElement;
    expect(llmApiKeyInput.value).toBe("");
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("routes shared openai_api_key replacement by readiness instead of forcing OpenAI Realtime", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        asr_provider: { type: "local_whisper" },
        llm_provider: { type: "local_llama" },
      },
    });
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "asr.openai_realtime",
        status: "ready",
        message: "OpenAI Realtime key is valid",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openai_api_key", present: true }],
      },
      {
        provider_id: "llm.api",
        status: "ready",
        message: "OpenAI-compatible LLM key is valid",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openai_api_key", present: true }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openai_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    await waitFor(() =>
      expect(credentialHealthRowForKey("openai_api_key")).toBeInTheDocument(),
    );
    const row = credentialHealthRowForKey("openai_api_key");
    fireEvent.click(within(row).getByRole("button", { name: /replace/i }));

    await waitFor(() =>
      expect(document.getElementById("llm-custom-api-key")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /language model/i }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("routes shared openai_api_key replacement to the active OpenAI Realtime STT field", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        asr_provider: {
          type: "openai_realtime",
          api_key: "",
          model: "gpt-realtime-whisper",
          language: null,
        },
        llm_provider: { type: "local_llama" },
      },
    });
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "llm.api",
        status: "ready",
        message: "OpenAI-compatible LLM key is valid",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openai_api_key", present: true }],
      },
      {
        provider_id: "asr.openai_realtime",
        status: "ready",
        message: "OpenAI Realtime key is valid",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openai_api_key", present: true }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openai_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    await waitFor(() =>
      expect(credentialHealthRowForKey("openai_api_key")).toBeInTheDocument(),
    );
    const row = credentialHealthRowForKey("openai_api_key");
    fireEvent.click(within(row).getByRole("button", { name: /replace/i }));

    await waitFor(() =>
      expect(document.getElementById("openai-realtime-api-key")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /speech-to-text/i }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("radio", { name: /openai realtime/i }),
    ).toBeChecked();
    expect(screen.getByText(/saved OpenAI key available/i)).toBeInTheDocument();
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("routes planned realtime_agent.openai_realtime credential replacement through the existing OpenAI Realtime field", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        asr_provider: { type: "local_whisper" },
        llm_provider: { type: "local_llama" },
      },
    });
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "realtime_agent.openai_realtime",
        status: "missing_credentials",
        message: "Missing saved credential(s): openai_api_key",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openai_api_key", present: true }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openai_api_key",
            present: true,
            source: "os_keychain",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    await waitFor(() =>
      expect(credentialHealthRowForKey("openai_api_key")).toBeInTheDocument(),
    );
    const row = credentialHealthRowForKey("openai_api_key");
    expect(
      within(row).getByText(/OpenAI Realtime voice agent: Missing key/i),
    ).toBeInTheDocument();

    fireEvent.click(within(row).getByRole("button", { name: /replace/i }));

    await waitFor(() =>
      expect(document.getElementById("openai-realtime-api-key")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /speech-to-text/i }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("radio", { name: /openai realtime/i }),
    ).toBeChecked();
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it.each([
    {
      key: "soniox_api_key",
      providerId: "asr.soniox",
      message: "Soniox realtime: Missing key",
    },
    {
      key: "gladia_api_key",
      providerId: "asr.gladia",
      message: "Gladia Solaria live: Missing key",
    },
    {
      key: "speechmatics_api_key",
      providerId: "asr.speechmatics",
      message: "Speechmatics realtime enhanced: Missing key",
    },
    {
      key: "elevenlabs_api_key",
      providerId: "asr.elevenlabs_scribe",
      message: "ElevenLabs Scribe realtime: Missing key",
    },
    {
      key: "revai_api_key",
      providerId: "asr.revai",
      message: "Rev AI realtime: Missing key",
    },
  ])("keeps planned provider-only credential slot $key without a settings input unrouted", async ({
    key,
    providerId,
    message,
  }) => {
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key,
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: providerId,
            status: "missing_credentials",
            message: `Missing saved credential(s): ${key}`,
            checked_at: null,
            stale: false,
            credential_epoch: 0,
            credentials: [{ key, present: true }],
          },
        ];
      }
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    await waitFor(() =>
      expect(credentialHealthRowForKey(key)).toBeInTheDocument(),
    );
    const row = credentialHealthRowForKey(key);
    expect(row).toHaveTextContent(message);
    expect(
      within(row).queryByRole("button", { name: /replace/i }),
    ).not.toBeInTheDocument();
  });

  it("routes shared openai_api_key replacement to the active STT API field", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        asr_provider: {
          type: "api",
          endpoint: "https://api.openai.com/v1",
          api_key: "",
          model: "whisper-1",
        },
        llm_provider: { type: "local_llama" },
      },
    });
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "llm.api",
        status: "ready",
        message: "OpenAI-compatible LLM key is valid",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openai_api_key", present: true }],
      },
      {
        provider_id: "asr.api",
        status: "ready",
        message: "OpenAI-compatible ASR key is valid",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openai_api_key", present: true }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openai_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    await waitFor(() =>
      expect(credentialHealthRowForKey("openai_api_key")).toBeInTheDocument(),
    );
    const row = credentialHealthRowForKey("openai_api_key");
    fireEvent.click(within(row).getByRole("button", { name: /replace/i }));

    await waitFor(() =>
      expect(document.getElementById("asr-api-key")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /speech-to-text/i }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("radio", { name: /openai-compatible batch asr/i }),
    ).toBeChecked();
    expect(
      screen.getByText(/saved key available for this endpoint/i),
    ).toBeInTheDocument();
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("prioritizes the active provider path even before checks have run", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        llm_provider: { type: "local_llama" },
      },
    });
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "asr.deepgram",
        status: "unchecked",
        message: "Deepgram has not been checked",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [],
      },
      {
        provider_id: "asr.local_whisper",
        status: "unchecked",
        message: "Local Whisper has not been checked",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [],
      },
      {
        provider_id: "llm.local_llama",
        status: "unchecked",
        message: "Local llama.cpp has not been checked",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [],
      },
      {
        provider_id: "realtime_agent.gemini_live",
        status: "unchecked",
        message: "Gemini Live has not been checked",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [],
      },
      {
        provider_id: "tts.none",
        status: "unchecked",
        message: "Text-to-speech is disabled",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [],
      },
      {
        provider_id: "llm.openrouter",
        status: "missing_credentials",
        message: "Missing saved credential(s): openrouter_api_key",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openrouter_api_key", present: false }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") return [];
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    const { container } = render(<SettingsPage />);

    await readinessRowForProvider(/^Local Whisper$/i);
    const providerLabels = Array.from(
      container.querySelectorAll(".settings-readiness__provider"),
      (node) => node.textContent,
    );

    expect(providerLabels.slice(0, 3)).toEqual([
      "Local Whisper",
      "Local llama.cpp",
      "TTS disabled",
    ]);
    expect(screen.getByText("Selected: ggml-small.en.bin")).toBeInTheDocument();
    expect(screen.getByText("Selected: llama3.2")).toBeInTheDocument();
    expect(
      screen.queryByText("Selected: gemini-3.1-flash-live-preview"),
    ).not.toBeInTheDocument();
    expect(providerLabels).toContain("OpenRouter");
    expect(providerLabels).not.toContain("Deepgram streaming");
  });

  it("opens the STT replace-key field from a missing readiness row without loading plaintext", async () => {
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "asr.deepgram",
        status: "missing_credentials",
        message: "Missing saved credential(s): deepgram_api_key",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "deepgram_api_key", present: false }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") return [];
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);
    const row = await readinessRowForProvider(/^Deepgram streaming$/i);

    fireEvent.click(
      within(row).getByRole("button", { name: /add or replace credential/i }),
    );

    await waitFor(() =>
      expect(document.getElementById("deepgram-api-key")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /speech-to-text/i }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("radio", { name: /^deepgram streaming$/i }),
    ).toBeChecked();
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("opens the LLM replace-key field from a saved-key readiness row", async () => {
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "llm.openrouter",
        status: "error",
        message: "401 Unauthorized",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "openrouter_api_key", present: true }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openrouter_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);
    const row = await readinessRowForProvider(/^OpenRouter$/i);

    fireEvent.click(
      within(row).getByRole("button", { name: /add or replace credential/i }),
    );

    await waitFor(() =>
      expect(document.getElementById("llm-openrouter-api-key")).toHaveFocus(),
    );
    expect(
      screen.getByRole("tab", { name: /language model/i }),
    ).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("radio", { name: /openrouter/i })).toBeChecked();
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("does not offer credential-field routing for credentialless planned local providers", async () => {
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "asr.moonshine",
        status: "unchecked",
        message:
          "Local model files ready: 1/3 model option(s). Provider runtime remains planned and is not selectable yet.",
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [],
        runtime: {
          status: "runtime_unavailable",
          message:
            "Moonshine native runtime adapter is not wired yet; provider remains planned and unselectable.",
          required_feature: null,
          runtime_version: null,
          model_id: "moonshine-small-streaming-en",
        },
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") return [];
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);
    const row = await readinessRowForProvider(/^Moonshine local streaming$/i);

    expect(
      within(row).queryByRole("button", { name: /open credential field/i }),
    ).not.toBeInTheDocument();
  });

  it("does not route shared TTS Deepgram readiness through the STT provider", async () => {
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "tts.deepgram_aura",
        status: "error",
        message: "Deepgram Aura key failed validation",
        checked_at: Date.now(),
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "deepgram_api_key", present: true }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "deepgram_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);
    const row = await readinessRowForProvider(/^Deepgram Aura$/i);

    expect(
      within(row).queryByRole("button", { name: /open credential field/i }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /general/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("does not ask Gemini Vertex saved credentials to run unsupported checks", async () => {
    const readiness: ProviderReadiness[] = [
      {
        provider_id: "realtime_agent.gemini_live",
        status: "unchecked",
        message: "Vertex AI readiness is not probed automatically yet",
        automatic_probe_available: false,
        checked_at: null,
        stale: false,
        credential_epoch: 0,
        credentials: [{ key: "google_service_account_path", present: true }],
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "google_service_account_path",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return readiness;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);
    const row = await readinessRowForProvider(/^Gemini Live$/i);

    expect(
      within(row).queryByText(/run checks to validate/i),
    ).not.toBeInTheDocument();
    expect(
      within(row).queryByRole("button", { name: /open credential field/i }),
    ).not.toBeInTheDocument();
  });

  it("refreshes provider readiness after saving settings", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });

    render(<SettingsPage />);

    await waitFor(() => expect(providerReadinessCalls()).toHaveLength(1));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
    });

    await waitFor(() =>
      expect(providerReadinessCalls().length).toBeGreaterThanOrEqual(2),
    );
    expect(providerReadinessCalls().at(-1)).toEqual([
      "get_provider_readiness_cmd",
      notesReadinessArgs(true),
    ]);
  });

  it("renders saved provider readiness and hydrates OpenRouter models from the backend", async () => {
    const fixtureModels = [
      {
        id: "anthropic/claude-sonnet-4.5",
        name: "Anthropic: Claude Sonnet 4.5",
        context_length: 200000,
        pricing: { prompt: "0.000003", completion: "0.000015" },
      },
    ];
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openrouter_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback(args);
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "asr.assemblyai",
            status: "ready",
            message:
              "AssemblyAI account key is valid via REST; v3 streaming socket smoke not run",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "assemblyai_api_key", present: true }],
            model_catalog: [
              {
                id: "universal-3-5-pro",
                display_name: "Universal-3.5 Pro Streaming",
                is_default: true,
              },
            ],
          },
          {
            provider_id: "asr.soniox",
            status: "ready",
            message: "Soniox API key is valid (1 real-time STT models)",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "soniox_api_key", present: true }],
            model_count: 1,
            model_catalog: [
              {
                id: "stt-rt-v5",
                display_name: "Speech-to-Text Real-time v5",
                is_default: true,
              },
            ],
          },
          {
            provider_id: "asr.deepgram",
            status: "missing_credentials",
            message: "Missing saved credential(s): deepgram_api_key",
            checked_at: null,
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "deepgram_api_key", present: false }],
          },
          {
            provider_id: "asr.openai_realtime",
            status: "ready",
            message: "OpenAI Realtime key is valid",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "openai_api_key", present: true }],
            model_catalog: [
              {
                id: "gpt-realtime-whisper",
                display_name: "GPT Realtime Whisper",
                is_default: true,
              },
            ],
          },
          {
            provider_id: "llm.openrouter",
            status: "ready",
            message: "OpenRouter API key is valid (1 models)",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "openrouter_api_key", present: true }],
            model_count: 1,
            openrouter_models: fixtureModels,
          },
          {
            provider_id: "llm.mistralrs",
            status: "unchecked",
            message: "Local model readiness is checked by the model manager",
            checked_at: null,
            stale: false,
            credential_epoch: 0,
            credentials: [],
            model_catalog: [
              {
                id: "lfm2-350m-extract-q4_k_m.gguf",
                display_name: "LFM2 extract Q4",
                is_default: true,
              },
            ],
          },
          {
            provider_id: "asr.moonshine",
            status: "unchecked",
            message:
              "Local model files ready: 1/3 model option(s). Provider runtime remains planned and is not selectable yet.",
            checked_at: null,
            stale: false,
            credential_epoch: 0,
            credentials: [],
            model_catalog: [
              {
                id: "moonshine-small-streaming-en",
                display_name: "moonshine-small-streaming-en",
                is_default: true,
              },
            ],
            runtime: {
              status: "runtime_unavailable",
              message:
                "Moonshine native runtime adapter is not wired yet; provider remains planned and unselectable.",
              required_feature: null,
              runtime_version: null,
              model_id: "moonshine-small-streaming-en",
            },
          },
          {
            provider_id: "realtime_agent.gemini_live",
            status: "ready",
            message: "Gemini API key is valid",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "gemini_api_key", present: true }],
            model_catalog: [
              {
                id: "gemini-2.0-flash-live-001",
                display_name: "Gemini 2.0 Flash Live",
                is_default: true,
              },
            ],
          },
          {
            provider_id: "tts.deepgram_aura",
            status: "ready",
            message: "Deepgram Aura key is valid",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "deepgram_api_key", present: true }],
            model_count: 12,
            voice_catalog: auraVoiceCatalogFixture(),
          },
        ];
      }
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);

    expect(
      await screen.findByRole("heading", { name: /provider readiness/i }),
    ).toBeInTheDocument();
    const assemblyAiReadinessRow = await readinessRowForProvider(
      /^AssemblyAI streaming$/i,
    );
    expect(
      within(assemblyAiReadinessRow).getByText(/^AssemblyAI streaming$/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /AssemblyAI account key is valid via REST; v3 streaming socket smoke not run.*Catalog: 1 models/i,
      ),
    ).toBeInTheDocument();
    const sonioxReadinessRow =
      await readinessRowForProvider(/^Soniox realtime$/i);
    expect(
      within(sonioxReadinessRow).getByText(/^Soniox realtime$/i),
    ).toBeInTheDocument();
    expect(
      within(sonioxReadinessRow).getByText(
        /Soniox API key is valid.*Catalog: 1 models/i,
      ),
    ).toBeInTheDocument();
    const moonshineReadinessRow = await readinessRowForProvider(
      /^Moonshine local streaming$/i,
    );
    fireEvent.click(within(moonshineReadinessRow).getByText(/details/i));
    expect(
      within(moonshineReadinessRow).getByText(/runtime unavailable/i),
    ).toBeInTheDocument();
    expect(
      within(moonshineReadinessRow).getByText(
        /native runtime adapter is not wired/i,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/add the missing key in this provider section/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/run checks to validate/i),
    ).not.toBeInTheDocument();
    const openrouterReadinessRow =
      await readinessRowForProvider(/^OpenRouter$/i);
    expect(
      within(openrouterReadinessRow).getByText(/OpenRouter API key is valid/i),
    ).toBeInTheDocument();
    fireEvent.click(within(openrouterReadinessRow).getByText(/details/i));
    expect(
      within(openrouterReadinessRow).getByText("openrouter_api_key"),
    ).toBeInTheDocument();
    expect(
      within(openrouterReadinessRow).getByText(/credentials\.yaml/i),
    ).toBeInTheDocument();
    expect(
      within(openrouterReadinessRow).getByText(/last checked/i),
    ).toBeInTheDocument();

    goToTab(/language model/i);
    fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
    const llmGroup = screen
      .getByRole("heading", { name: /LLM Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(readinessStatus(llmGroup)).toHaveTextContent(
      /OpenRouter API key is valid.*Catalog: 1 models/i,
    );
    expect(readinessStatus(llmGroup)).toHaveTextContent(
      /Data\s*Configured endpoint.*Session\s*Per request.*Auth\s*Saved key.*Close\s*Request completes/i,
    );
    fireEvent.click(within(llmGroup).getByText(/details/i));
    expect(
      within(llmGroup).getByText("openrouter_api_key"),
    ).toBeInTheDocument();
    expect(
      within(llmGroup).getByText(/credentials\.yaml/i),
    ).toBeInTheDocument();
    const openrouterPicker = within(llmGroup).getByRole("combobox", {
      name: /openrouter model/i,
    });
    fireEvent.focus(openrouterPicker);
    expect(
      await screen.findByText(/Anthropic: Claude Sonnet 4\.5/),
    ).toBeInTheDocument();
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd, args]) =>
          cmd === "load_credential_cmd" &&
          (args as { key?: string } | undefined)?.key === "openrouter_api_key",
      ),
    ).toBe(false);
    expect(screen.queryByText("sk-or-should-not-load")).not.toBeInTheDocument();

    goToTab(/speech-to-text/i);
    expect(
      screen.queryByRole("radio", { name: /moonshine local streaming/i }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: /openai realtime/i }));
    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(readinessStatus(asrGroup)).toHaveTextContent(
      /OpenAI Realtime key is valid.*Catalog: 1 models/i,
    );
    expect(readinessStatus(asrGroup)).toHaveTextContent(
      /Data\s*Vendor cloud.*Session\s*WebSocket.*Auth\s*Saved key.*Keepalive\s*Audio stream.*Close\s*End stream \+ close/i,
    );
    const openaiRealtimePicker = within(asrGroup).getByRole("combobox", {
      name: /^model$/i,
    });
    fireEvent.focus(openaiRealtimePicker);
    expect(
      within(asrGroup).getByRole("option", {
        name: /GPT Realtime Whisper \(gpt-realtime-whisper\)/i,
      }),
    ).toBeInTheDocument();

    goToTab(/language model/i);
    fireEvent.click(screen.getByRole("radio", { name: /mistral/i }));
    const mistralGroup = screen
      .getByRole("heading", { name: /LLM Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    const mistralPicker = within(mistralGroup).getByRole("combobox", {
      name: /model id/i,
    });
    fireEvent.focus(mistralPicker);
    expect(
      within(mistralGroup).getByRole("option", {
        name: /LFM2 extract Q4 \(lfm2-350m-extract-q4_k_m\.gguf\)/i,
      }),
    ).toBeInTheDocument();

    goToTab(/gemini/i);
    const geminiSection = screen
      .getByRole("heading", { name: /Gemini Live/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(readinessStatus(geminiSection)).toHaveTextContent(
      /Gemini API key is valid.*Catalog: 1 models/i,
    );
    expect(readinessStatus(geminiSection)).toHaveTextContent(
      /Data\s*Provider account.*Session\s*WebSocket.*Auth\s*Google auth.*Keepalive\s*Audio stream.*Close\s*End stream \+ close/i,
    );
    const geminiPicker = within(geminiSection).getByRole("combobox", {
      name: /^model$/i,
    });
    fireEvent.focus(geminiPicker);
    expect(
      within(geminiSection).getByRole("option", {
        name: /Gemini 2\.0 Flash Live \(gemini-2\.0-flash-live-001\)/i,
      }),
    ).toBeInTheDocument();

    goToTab(/text-to-speech/i);
    fireEvent.change(screen.getByLabelText(/^provider$/i), {
      target: { value: "deepgram_aura" },
    });
    const ttsGroup = screen
      .getByRole("heading", { name: /Text-to-Speech/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(readinessStatus(ttsGroup)).toHaveTextContent(
      /Deepgram Aura key is valid.*Catalog: 12 voices/i,
    );
    expect(readinessStatus(ttsGroup)).toHaveTextContent(
      /Data\s*Vendor cloud.*Session\s*WebSocket.*Auth\s*Saved key.*Keepalive\s*Control message.*Close\s*Provider close/i,
    );
    expect(
      within(ttsGroup).getByRole("combobox", { name: /^voice$/i }),
    ).toHaveValue("aura-asteria-en");
  });

  it("refreshes provider readiness after clearing a saved credential", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    resetStore({
      settings: {
        ...baseSettings,
        llm_provider: {
          type: "openrouter",
          model: "",
          base_url: "https://openrouter.ai/api/v1",
          provider_order: null,
          include_usage_in_stream: true,
          api_key: "",
        },
      },
    });
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openrouter_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "llm.openrouter",
            status: "ready",
            message: "OpenRouter API key is valid",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "openrouter_api_key", present: true }],
          },
        ];
      }
      if (cmd === "delete_credential_cmd") return undefined;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/language model/i);

    await waitFor(() => expect(providerReadinessCalls()).toHaveLength(1));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /clear saved key/i }));
    });

    expect(mockedInvoke).toHaveBeenCalledWith("delete_credential_cmd", {
      key: "openrouter_api_key",
    });
    await waitFor(() =>
      expect(providerReadinessCalls().length).toBeGreaterThanOrEqual(2),
    );
    expect(providerReadinessCalls().at(-1)).toEqual([
      "get_provider_readiness_cmd",
      notesReadinessArgs(true),
    ]);
    confirmSpy.mockRestore();
  });

  it("wires Settings tabs to tabpanels and supports keyboard navigation", () => {
    render(<SettingsPage />);

    const generalTab = screen.getByRole("tab", { name: /general/i });
    const sttTab = screen.getByRole("tab", { name: /speech-to-text/i });
    const loggingTab = screen.getByRole("tab", { name: /logging/i });
    const generalPanel = screen.getByRole("tabpanel", { name: /general/i });

    expect(
      screen.getByRole("tablist", { name: /^settings$/i }),
    ).toBeInTheDocument();
    expect(generalTab).toHaveAttribute("aria-selected", "true");
    expect(generalTab).toHaveAttribute("tabindex", "0");
    expect(sttTab).toHaveAttribute("tabindex", "-1");
    expect(generalTab).toHaveAttribute("aria-controls", generalPanel.id);
    expect(generalPanel).toHaveAttribute("aria-labelledby", generalTab.id);

    generalTab.focus();
    fireEvent.keyDown(generalTab, { key: "ArrowRight" });

    const sttPanel = screen.getByRole("tabpanel", {
      name: /speech-to-text/i,
    });
    expect(sttTab).toHaveFocus();
    expect(sttTab).toHaveAttribute("aria-selected", "true");
    expect(sttTab).toHaveAttribute("tabindex", "0");
    expect(sttTab).toHaveAttribute("aria-controls", sttPanel.id);
    expect(sttPanel).toHaveAttribute("aria-labelledby", sttTab.id);

    fireEvent.keyDown(sttTab, { key: "End" });
    expect(loggingTab).toHaveFocus();
    expect(loggingTab).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("tabpanel", { name: /logging/i }),
    ).toBeInTheDocument();
  });

  it("shows all section headings (Audio, Models, ASR, LLM, Gemini, Diagnostics)", () => {
    render(<SettingsPage />);
    // Sections are now behind tabs. The tab bar exposes each group; the
    // General tab (default) shows Audio + Models/Diagnostics.
    expect(screen.getByRole("tab", { name: /general/i })).toBeInTheDocument();
    expect(
      screen.getByRole("tab", { name: /speech-to-text/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("tab", { name: /language model/i }),
    ).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /gemini/i })).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: /^audio$/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: /^models$/i }),
    ).toBeInTheDocument();
    // Switch to each provider tab and confirm its heading renders.
    goToTab(/speech-to-text/i);
    expect(
      screen.getByRole("heading", { name: /ASR Provider/i, level: 3 }),
    ).toBeInTheDocument();
    goToTab(/language model/i);
    expect(
      screen.getByRole("heading", { name: /LLM Provider/i, level: 3 }),
    ).toBeInTheDocument();
    goToTab(/gemini/i);
    expect(
      screen.getByRole("heading", { name: /gemini live/i, level: 3 }),
    ).toBeInTheDocument();
  });

  it("AudioSettings sample-rate dropdown exposes all six allowed rates", () => {
    render(<SettingsPage />);
    const select = screen.getByLabelText(
      /capture sample rate/i,
    ) as HTMLSelectElement;
    const values = Array.from(select.options).map((o) => o.value);
    expect(values).toEqual([
      "22050",
      "32000",
      "44100",
      "48000",
      "88200",
      "96000",
    ]);
  });

  it("changing the sample-rate dropdown updates the selected value", () => {
    render(<SettingsPage />);
    const select = screen.getByLabelText(
      /capture sample rate/i,
    ) as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "48000" } });
    expect(select.value).toBe("48000");
  });

  it("AsrProviderSettings Local Whisper hides the cloud-API ASR endpoint field", () => {
    render(<SettingsPage />);
    // Default ASR is local_whisper, so the ASR-section Cloud API branch
    // (which is keyed on the api.openai.com/v1 endpoint placeholder)
    // must not render. The LLM section uses sk-... independently.
    expect(
      screen.queryByPlaceholderText("https://api.openai.com/v1"),
    ).not.toBeInTheDocument();
    expect(screen.queryByPlaceholderText("whisper-1")).not.toBeInTheDocument();
  });

  it("selecting Cloud API for ASR reveals endpoint + credential action + model inputs", () => {
    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    const cloudRadio = screen.getByRole("radio", {
      name: /openai-compatible batch asr/i,
    });
    fireEvent.click(cloudRadio);
    expect(
      screen.getByPlaceholderText("https://api.openai.com/v1"),
    ).toBeInTheDocument();
    const asrGroup = settingsSectionForHeading(/ASR Provider/i);
    expect(
      within(asrGroup).queryByLabelText(/^api key$/i),
    ).not.toBeInTheDocument();
    expect(
      within(asrGroup).getByRole("button", { name: /add key/i }),
    ).toBeInTheDocument();
    expect(screen.getByPlaceholderText("whisper-1")).toBeInTheDocument();
  });

  it("selecting AWS Transcribe reveals region + language-code inputs", () => {
    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    fireEvent.click(screen.getByRole("radio", { name: /aws transcribe/i }));
    // Both AWS sections default region placeholder to us-east-1; the ASR
    // section specifically also exposes a Language Code label.
    expect(
      screen.getAllByPlaceholderText("us-east-1").length,
    ).toBeGreaterThanOrEqual(1);
    expect(screen.getByPlaceholderText("en-US")).toBeInTheDocument();
  });

  it("OpenAI-compatible LLM uses the shared model picker and saves a custom model when no catalog is available", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });
    render(<SettingsPage />);
    goToTab(/language model/i);
    // Default state is already llmType === "api", so the fields render.
    expect(
      screen.getByPlaceholderText("http://localhost:8000/v1"),
    ).toBeInTheDocument();
    const llmGroup = screen
      .getByRole("heading", { name: /LLM Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    const modelPicker = within(llmGroup).getByRole("combobox", {
      name: /^model$/i,
    }) as HTMLInputElement;

    expect(modelPicker).toHaveAttribute("aria-autocomplete", "list");
    expect(
      within(llmGroup).getByText(/No catalog loaded\. Type a custom model id/i),
    ).toBeInTheDocument();

    fireEvent.change(modelPicker, {
      target: { value: "custom/openai-compatible-chat" },
    });
    expect(modelPicker).toHaveValue("custom/openai-compatible-chat");

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
    });

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0]?.[0];
    expect(saved.llm_provider).toMatchObject({
      type: "api",
      model: "custom/openai-compatible-chat",
    });
  });

  it("keeps OpenAI-compatible LLM tuning behind Advanced and still saves it", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });
    render(<SettingsPage />);
    goToTab(/language model/i);

    const llmGroup = screen
      .getByRole("heading", { name: /LLM Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    const summary = within(llmGroup).getByText(/advanced provider controls/i);
    const advanced = summary.closest("details") as HTMLDetailsElement;
    expect(advanced.open).toBe(false);
    fireEvent.click(summary);
    expect(advanced.open).toBe(true);

    fireEvent.change(within(advanced).getByLabelText(/max tokens/i), {
      target: { value: "4096" },
    });
    fireEvent.change(within(advanced).getByLabelText(/temperature/i), {
      target: { value: "0.2" },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
    });

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const arg = saveSettings.mock.calls[0]?.[0];
    expect(arg.llm_provider.type).toBe("api");
    expect(arg.llm_api_config).toMatchObject({
      max_tokens: 4096,
      temperature: 0.2,
    });
  });

  it("does not hydrate generic OpenRouter-compatible LLM endpoints from the saved OpenRouter key", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        llm_provider: {
          type: "api",
          endpoint: "https://openrouter.ai/api/v1",
          api_key: "",
          model: "openai/gpt-5.2",
        },
      },
    });
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "openrouter_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback(args);
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/language model/i);

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("load_credential_presence_cmd"),
    );
    const llmGroup = screen
      .getByRole("heading", { name: /LLM Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(
      within(llmGroup).getByText(/saved key available for this endpoint/i),
    ).toBeInTheDocument();
    expect(
      within(llmGroup).queryByLabelText(/^api key$/i),
    ).not.toBeInTheDocument();
    expect(
      within(llmGroup).getByRole("button", { name: /replace key/i }),
    ).toBeInTheDocument();
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd, args]) =>
          cmd === "load_credential_cmd" &&
          (args as { key?: string } | undefined)?.key === "openrouter_api_key",
      ),
    ).toBe(false);
  });

  it("does not hydrate legacy plaintext provider settings into replacement fields", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        asr_provider: {
          type: "api",
          endpoint: "https://api.openai.com/v1",
          api_key: "sk-legacy-asr",
          model: "whisper-1",
        },
        llm_provider: {
          type: "openrouter",
          api_key: "sk-or-legacy-llm",
          model: "openai/gpt-5.2",
          base_url: "https://openrouter.ai/api/v1",
          provider_order: null,
          include_usage_in_stream: true,
        },
        gemini: {
          auth: {
            type: "api_key",
            api_key: "AIza-legacy-gemini",
          },
          model: "gemini-2.0-flash-live-001",
        },
      },
    });

    render(<SettingsPage />);

    goToTab(/speech-to-text/i);
    const asrGroup = settingsSectionForHeading(/ASR Provider/i);
    const asrKeyInput = await openCredentialInput(asrGroup, /^api key$/i);
    expect(asrKeyInput).toHaveValue("");

    goToTab(/language model/i);
    const llmGroup = settingsSectionForHeading(/LLM Provider/i);
    const openrouterKeyInput = await openCredentialInput(
      llmGroup,
      /openrouter api key/i,
    );
    expect(openrouterKeyInput).toHaveValue("");

    goToTab(/gemini/i);
    const geminiSection = settingsSectionForHeading(/Gemini Live/i);
    const geminiKeyInput = await openCredentialInput(
      geminiSection,
      /gemini api key/i,
    );
    expect(geminiKeyInput).toHaveValue("");

    expect(screen.queryByDisplayValue("sk-legacy-asr")).not.toBeInTheDocument();
    expect(
      screen.queryByDisplayValue("sk-or-legacy-llm"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByDisplayValue("AIza-legacy-gemini"),
    ).not.toBeInTheDocument();
  });

  it("uses a saved generic ASR endpoint key for testing without hydrating it into the field", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        asr_provider: {
          type: "api",
          endpoint: "https://api.groq.com/openai/v1",
          api_key: "",
          model: "whisper-large-v3",
        },
      },
    });
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "groq_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback(args);
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "asr.api",
            status: "ready",
            message: "Connected to https://api.groq.com/openai/v1 (1 models)",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "groq_api_key", present: true }],
            model_count: 1,
            model_catalog: [
              {
                id: "whisper-large-v3",
                display_name: "Whisper Large v3",
                is_default: false,
              },
            ],
          },
        ];
      }
      if (cmd === "test_cloud_asr_connection") return "Connected";
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/speech-to-text/i);

    expect(
      await screen.findByText(/saved key available for this endpoint/i),
    ).toBeInTheDocument();
    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(
      within(asrGroup).queryByLabelText(/^api key$/i),
    ).not.toBeInTheDocument();
    expect(
      within(asrGroup).getByRole("button", { name: /replace key/i }),
    ).toBeInTheDocument();
    const modelInput = within(asrGroup).getByRole("combobox", {
      name: /^model$/i,
    }) as HTMLInputElement;
    await waitFor(() => expect(modelInput).toBeInTheDocument());
    fireEvent.focus(modelInput);
    expect(
      within(asrGroup).getByRole("option", {
        name: /Whisper Large v3 \(whisper-large-v3\)/i,
      }),
    ).toBeInTheDocument();

    await act(async () => {
      fireEvent.click(
        within(asrGroup).getByRole("button", { name: /test connection/i }),
      );
    });

    expect(mockedInvoke).toHaveBeenCalledWith("test_cloud_asr_connection", {
      endpoint: "https://api.groq.com/openai/v1",
      apiKey: null,
    });
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd, args]) =>
          cmd === "load_credential_cmd" &&
          (args as { key?: string } | undefined)?.key === "groq_api_key",
      ),
    ).toBe(false);
  });

  it("does not hydrate generic LLM endpoint keys from the credential store", async () => {
    resetStore({
      settings: {
        ...baseSettings,
        llm_provider: {
          type: "api",
          endpoint: "https://api.groq.com/openai/v1",
          api_key: "",
          model: "llama-3.3-70b-versatile",
        },
      },
    });
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "groq_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback(args);
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/language model/i);

    expect(
      await screen.findByText(/saved key available for this endpoint/i),
    ).toBeInTheDocument();
    const llmGroup = screen
      .getByRole("heading", { name: /LLM Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(
      within(llmGroup).queryByLabelText(/^api key$/i),
    ).not.toBeInTheDocument();
    expect(
      within(llmGroup).getByRole("button", { name: /replace key/i }),
    ).toBeInTheDocument();
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd, args]) =>
          cmd === "load_credential_cmd" &&
          (args as { key?: string } | undefined)?.key === "groq_api_key",
      ),
    ).toBe(false);
  });

  it("selecting AWS Bedrock keeps region and model visible with credentials behind Advanced", () => {
    render(<SettingsPage />);
    goToTab(/language model/i);
    fireEvent.click(screen.getByRole("radio", { name: /aws bedrock/i }));
    const llmGroup = screen
      .getByRole("heading", { name: /LLM Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(
      screen.getByPlaceholderText("anthropic.claude-3-haiku-20240307-v1:0"),
    ).toBeInTheDocument();
    const advanced = openSettingsAdvancedControls(llmGroup);
    expect(
      within(advanced).getByLabelText(/credential mode/i),
    ).toHaveDisplayValue(/default chain/i);
  });

  it("GeminiSettings renders auth-mode radios + model input", () => {
    render(<SettingsPage />);
    goToTab(/gemini/i);
    // Two Gemini auth radios: API Key vs Vertex AI.
    expect(
      screen.getByRole("radio", { name: /AI Studio \(API Key\)/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("radio", { name: /vertex ai/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByPlaceholderText("gemini-2.0-flash-live-001"),
    ).toBeInTheDocument();
  });

  it("uses a saved Gemini key for testing without hydrating it into the field", async () => {
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "gemini_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback(args);
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "test_gemini_api_key") return "Gemini API key is valid";
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/gemini/i);

    expect(
      await screen.findByText(/saved Gemini API key available/i),
    ).toBeInTheDocument();
    const geminiSection = screen
      .getByRole("heading", { name: /Gemini Live/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(
      within(geminiSection).queryByPlaceholderText("AIza..."),
    ).not.toBeInTheDocument();
    expect(
      within(geminiSection).getByRole("button", { name: /replace key/i }),
    ).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(
        within(geminiSection).getByRole("button", {
          name: /test connection/i,
        }),
      );
    });

    expect(mockedInvoke).toHaveBeenCalledWith("test_gemini_api_key", {
      apiKey: null,
    });
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd, args]) =>
          cmd === "load_credential_cmd" &&
          (args as { key?: string } | undefined)?.key === "gemini_api_key",
      ),
    ).toBe(false);
  });

  it("uses a saved Deepgram key for STT testing without hydrating it into the field", async () => {
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "deepgram_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback(args);
      if (cmd === "get_provider_readiness_cmd") {
        return [
          {
            provider_id: "asr.deepgram",
            status: "ready",
            message: "Deepgram API key is valid (2 streaming STT models)",
            checked_at: Date.now(),
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "deepgram_api_key", present: true }],
            model_count: 2,
            model_catalog: [
              {
                id: "nova-3",
                display_name: "nova-3",
                is_default: true,
              },
              {
                id: "flux-general-en",
                display_name: "Flux General English",
                is_default: false,
              },
            ],
          },
        ];
      }
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "test_deepgram_connection") {
        return "Deepgram API key is valid";
      }
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    fireEvent.click(
      screen.getByRole("radio", { name: /^deepgram streaming$/i }),
    );

    expect(
      await screen.findByText(/saved Deepgram key available/i),
    ).toBeInTheDocument();
    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(
      within(asrGroup).queryByPlaceholderText("dg-..."),
    ).not.toBeInTheDocument();
    expect(
      within(asrGroup).getByRole("button", { name: /replace key/i }),
    ).toBeInTheDocument();
    const modelInput = within(asrGroup).getByRole("combobox", {
      name: /^model$/i,
    }) as HTMLInputElement;
    fireEvent.focus(modelInput);
    expect(
      within(asrGroup).getByRole("option", {
        name: /Flux General English \(flux-general-en\)/i,
      }),
    ).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(
        within(asrGroup).getByRole("button", { name: /test connection/i }),
      );
    });

    expect(mockedInvoke).toHaveBeenCalledWith("test_deepgram_connection", {
      apiKey: null,
    });
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd, args]) =>
          cmd === "load_credential_cmd" &&
          (args as { key?: string } | undefined)?.key === "deepgram_api_key",
      ),
    ).toBe(false);
  });

  it("keeps Deepgram expert controls behind Advanced and still saves them", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });

    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    fireEvent.click(
      screen.getByRole("radio", { name: /^deepgram streaming$/i }),
    );

    const summary = screen.getAllByText(/advanced provider controls/i, {
      selector: "summary",
    })[1];
    const advanced = summary.closest("details") as HTMLDetailsElement;
    expect(advanced).toBeTruthy();
    expect(advanced.open).toBe(false);

    fireEvent.click(summary);
    expect(advanced.open).toBe(true);

    fireEvent.change(screen.getByLabelText(/deepgram endpointing/i), {
      target: { value: "450" },
    });
    fireEvent.change(screen.getByLabelText(/max speakers/i), {
      target: { value: "3" },
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
    });

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const arg = saveSettings.mock.calls[0]?.[0];
    expect(arg.asr_provider.type).toBe("deepgram");
    if (arg.asr_provider.type === "deepgram") {
      expect(arg.asr_provider.endpointing_ms).toBe(450);
      expect(arg.asr_provider.max_speakers).toBe(3);
    }
  });

  it("renders global diarization controls and disables unavailable modes", () => {
    render(<SettingsPage />);
    goToTab(/speech-to-text/i);

    expect(
      screen.getByRole("heading", { name: /^diarization$/i }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(/diarization mode/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/speaker count/i)).toBeInTheDocument();

    expect(
      screen.getByRole("option", { name: /provider labels/i }),
    ).toBeDisabled();
    expect(
      screen.getByRole("option", { name: /local timeline/i }),
    ).toBeDisabled();
    expect(
      screen.getByRole("option", { name: /hybrid provider \+ local/i }),
    ).toBeDisabled();
    expect(
      screen.getByText(/not available for the current provider/i),
    ).toBeInTheDocument();
  });

  it("persists global diarization policy and suppresses provider labels when off", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({
      saveSettings,
      modelStatus: {
        whisper: "Ready",
        llm: "Ready",
        sortformer: "Ready",
      },
      settings: {
        ...baseSettings,
        asr_provider: {
          type: "deepgram",
          api_key: "",
          model: "nova-3",
          enable_diarization: true,
          endpointing_ms: 300,
          utterance_end_ms: 1000,
          vad_events: true,
          eot_threshold: 0.5,
          eager_eot_threshold: 0,
          eot_timeout_ms: 0,
          max_speakers: 0,
        },
        diarization: {
          mode: "provider",
          speaker_count: "auto",
          max_speakers: null,
        },
      },
    });

    render(<SettingsPage />);
    goToTab(/speech-to-text/i);

    fireEvent.change(screen.getByLabelText(/diarization mode/i), {
      target: { value: "off" },
    });
    fireEvent.change(screen.getByLabelText(/speaker count/i), {
      target: { value: "fixed" },
    });

    const advanced = screen
      .getAllByText(/advanced provider controls/i, { selector: "summary" })[0]
      .closest("details") as HTMLDetailsElement;
    fireEvent.click(
      within(advanced).getByText(/advanced provider controls/i, {
        selector: "summary",
      }),
    );
    fireEvent.change(screen.getByLabelText(/maximum speakers/i), {
      target: { value: "5" },
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
    });

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const arg = saveSettings.mock.calls[0]?.[0];
    expect(arg.diarization).toEqual({
      mode: "off",
      speaker_count: "fixed",
      max_speakers: 5,
    });
    expect(arg.asr_provider.type).toBe("deepgram");
    if (arg.asr_provider.type === "deepgram") {
      expect(arg.asr_provider.enable_diarization).toBe(false);
    }
  });

  it("uses a saved AssemblyAI key for STT testing without hydrating it into the field", async () => {
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "assemblyai_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback(args);
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "test_assemblyai_connection") {
        return "AssemblyAI account key is valid via REST; v3 streaming socket smoke not run";
      }
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    fireEvent.click(
      screen.getByRole("radio", { name: /^assemblyai streaming$/i }),
    );

    expect(
      await screen.findByText(/saved AssemblyAI key available/i),
    ).toBeInTheDocument();

    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    expect(
      within(asrGroup).queryByPlaceholderText(/AssemblyAI API key/i),
    ).not.toBeInTheDocument();
    expect(
      within(asrGroup).getByRole("button", { name: /replace key/i }),
    ).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(
        within(asrGroup).getByRole("button", { name: /test connection/i }),
      );
    });

    expect(mockedInvoke).toHaveBeenCalledWith("test_assemblyai_connection", {
      apiKey: null,
    });
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd, args]) =>
          cmd === "load_credential_cmd" &&
          (args as { key?: string } | undefined)?.key === "assemblyai_api_key",
      ),
    ).toBe(false);
  });

  it("renders TTS provider options and Aura default from the provider registry", () => {
    render(<SettingsPage />);
    goToTab(/text-to-speech/i);

    const providerSelect = screen.getByLabelText(
      /^provider$/i,
    ) as HTMLSelectElement;
    expect(
      Array.from(providerSelect.options).map((option) => option.text),
    ).toEqual(["TTS disabled", "Deepgram Aura"]);
    fireEvent.change(providerSelect, {
      target: { value: "deepgram_aura" },
    });

    const voicePicker = screen.getByRole("combobox", {
      name: /voice/i,
    }) as HTMLInputElement;
    expect(voicePicker).toHaveValue("aura-asteria-en");
    expect(voicePicker).toHaveAttribute("aria-autocomplete", "list");
    expect(
      screen.getByText(/Search the catalog or type a custom model id/i),
    ).toBeInTheDocument();
  });

  it("uses a saved Deepgram key for Aura TTS testing without re-entering it", async () => {
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          {
            key: "deepgram_api_key",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback(args);
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "test_tts_connection_cmd") {
        return "Deepgram Aura TTS credentials look valid";
      }
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/text-to-speech/i);
    fireEvent.change(screen.getByLabelText(/^provider$/i), {
      target: { value: "deepgram_aura" },
    });

    expect(
      await screen.findByText(/saved Deepgram key available/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/Save a Deepgram API key in the ASR section/i),
    ).not.toBeInTheDocument();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /test connection/i }));
    });

    expect(mockedInvoke).toHaveBeenCalledWith("test_tts_connection_cmd", {
      provider: "deepgram_aura",
      apiKey: null,
    });
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd, args]) =>
          cmd === "load_credential_cmd" &&
          (args as { key?: string } | undefined)?.key === "deepgram_api_key",
      ),
    ).toBe(false);
  });

  it("saves a Deepgram key from the TTS Aura section without changing the ASR provider", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") return [];
      if (cmd === "get_provider_readiness_cmd") return [];
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "save_credential_cmd") return undefined;
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/text-to-speech/i);
    fireEvent.change(screen.getByLabelText(/^provider$/i), {
      target: { value: "deepgram_aura" },
    });
    const ttsSection = settingsSectionForHeading(/Text-to-Speech/i);
    fireEvent.change(
      await openCredentialInput(ttsSection, /deepgram api key/i),
      {
        target: { value: "dg-tts-draft-key" },
      },
    );

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
    });

    expect(mockedInvoke).toHaveBeenCalledWith("save_credential_cmd", {
      key: "deepgram_api_key",
      value: "dg-tts-draft-key",
    });
    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0]?.[0];
    expect(saved.asr_provider).toEqual({ type: "local_whisper" });
    expect(saved.tts_provider).toEqual(
      expect.objectContaining({ type: "deepgram_aura" }),
    );
  });

  it("CredentialsManager renders the Models section header + empty state", () => {
    render(<SettingsPage />);
    expect(
      screen.getByRole("heading", { name: /^models$/i }),
    ).toBeInTheDocument();
    // models array is empty in the fixture — the empty state copy must
    // be visible so the user isn't left staring at a blank panel.
    expect(screen.getByText(/no models available/i)).toBeInTheDocument();
  });

  it("clicking Save invokes save_settings_cmd with the current state", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });
    render(<SettingsPage />);

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
    });

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const arg = saveSettings.mock.calls[0]?.[0];
    // Reducer default is local_whisper ASR + api LLM; Save must pass a
    // well-formed AppSettings shape to the store.
    expect(arg.asr_provider.type).toBe("local_whisper");
    expect(arg.llm_provider.type).toBe("api");
    expect(arg.audio_settings.sample_rate).toBe(48000);
    expect(arg.audio_settings.channels).toBe(2);
  });

  it("clicking the header ✕ button calls closeSettings", () => {
    const closeSettings = vi.fn();
    resetStore({ closeSettings });
    render(<SettingsPage />);
    fireEvent.click(screen.getByRole("button", { name: /close settings/i }));
    expect(closeSettings).toHaveBeenCalledTimes(1);
  });

  it("changing the backend log level triggers set_log_level on the backend", async () => {
    render(<SettingsPage />);
    const select = screen.getByLabelText(
      /backend log level/i,
    ) as HTMLSelectElement;
    await act(async () => {
      fireEvent.change(select, { target: { value: "debug" } });
    });
    expect(mockedInvoke).toHaveBeenCalledWith("set_log_level", {
      level: "debug",
    });
  });

  it("shows the loading placeholder when settingsLoading is true", () => {
    resetStore({ settingsLoading: true });
    render(<SettingsPage />);
    expect(screen.getByText(/loading settings/i)).toBeInTheDocument();
    // Sections are hidden behind the loading fallback.
    expect(
      screen.queryByRole("heading", { name: /^audio$/i }),
    ).not.toBeInTheDocument();
  });

  it("AWS Transcribe access-keys mode shares credentials with Bedrock via CLEAR_AWS_SHARED_KEYS", async () => {
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          { key: "aws_access_key", present: true, source: "credentials_yaml" },
          { key: "aws_secret_key", present: true, source: "credentials_yaml" },
        ];
      }
      if (cmd === "get_provider_readiness_cmd") return [];
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });
    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    fireEvent.click(screen.getByRole("radio", { name: /aws transcribe/i }));
    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    const advanced = openSettingsAdvancedControls(asrGroup);
    fireEvent.change(within(advanced).getByLabelText(/credential mode/i), {
      target: { value: "access_keys" },
    });
    // The "Clear Saved AWS Keys" button should now be visible — clicking
    // it triggers handleClearCredential → CLEAR_AWS_SHARED_KEYS.
    const clearBtn = await within(advanced).findByRole("button", {
      name: /clear saved aws keys/i,
    });
    expect(clearBtn).toBeInTheDocument();
  });

  it("testing AWS Transcribe access keys uses draft credentials without saving them", async () => {
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "test_aws_credentials") return "Authenticated as test";
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;

    fireEvent.click(
      within(asrGroup).getByRole("radio", { name: /aws transcribe/i }),
    );
    const advanced = openSettingsAdvancedControls(asrGroup);
    fireEvent.change(within(advanced).getByLabelText(/credential mode/i), {
      target: { value: "access_keys" },
    });
    fireEvent.change(
      await openCredentialInput(
        advanced,
        /access key id/i,
        /add aws keys|replace aws keys/i,
      ),
      {
        target: { value: "AKIA_TEST" },
      },
    );
    fireEvent.change(
      within(advanced).getByLabelText(/secret access key/i, {
        selector: "input",
      }),
      { target: { value: "secret-test" } },
    );
    fireEvent.change(
      within(advanced).getByLabelText(/session token/i, {
        selector: "input",
      }),
      { target: { value: "session-test" } },
    );

    await act(async () => {
      fireEvent.click(
        within(asrGroup).getByRole("button", { name: /test connection/i }),
      );
    });

    expect(mockedInvoke).toHaveBeenCalledWith("test_aws_credentials", {
      region: "us-east-1",
      credentialSource: { type: "access_keys", access_key: "AKIA_TEST" },
      secretAccessKey: "secret-test",
      sessionToken: "session-test",
    });
    expect(saveCredentialCalls()).toHaveLength(0);
  });

  it("uses saved AWS access keys for Transcribe testing without hydrating them", async () => {
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          { key: "aws_access_key", present: true, source: "credentials_yaml" },
          { key: "aws_secret_key", present: true, source: "credentials_yaml" },
          {
            key: "aws_session_token",
            present: true,
            source: "credentials_yaml",
          },
        ];
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback(args);
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "test_aws_credentials") return "Authenticated as test";
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;

    fireEvent.click(
      within(asrGroup).getByRole("radio", { name: /aws transcribe/i }),
    );
    const advanced = openSettingsAdvancedControls(asrGroup);
    fireEvent.change(within(advanced).getByLabelText(/credential mode/i), {
      target: { value: "access_keys" },
    });

    expect(
      await within(advanced).findByText(
        /Saved AWS access keys and session token/i,
      ),
    ).toBeInTheDocument();
    expect(
      within(advanced).queryByLabelText(/access key id/i, {
        selector: "input",
      }),
    ).not.toBeInTheDocument();
    expect(
      within(advanced).getByRole("button", { name: /replace aws keys/i }),
    ).toBeInTheDocument();
    expect(
      within(advanced).queryByLabelText(/secret access key/i, {
        selector: "input",
      }),
    ).not.toBeInTheDocument();

    await act(async () => {
      fireEvent.click(
        within(asrGroup).getByRole("button", { name: /test connection/i }),
      );
    });

    expect(mockedInvoke).toHaveBeenCalledWith("test_aws_credentials", {
      region: "us-east-1",
      credentialSource: { type: "access_keys", access_key: "" },
      secretAccessKey: null,
      sessionToken: null,
    });
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd, args]) =>
          cmd === "load_credential_cmd" &&
          String((args as { key?: string } | undefined)?.key ?? "").startsWith(
            "aws_",
          ),
      ),
    ).toBe(false);
  });

  it("testing AWS Bedrock access keys uses draft credentials without saving them", async () => {
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
      if (cmd === "list_aws_profiles") return [];
      if (cmd === "test_aws_credentials") return "Authenticated as test";
      return undefined;
    });

    render(<SettingsPage />);
    goToTab(/language model/i);
    const llmGroup = screen
      .getByRole("heading", { name: /LLM Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;

    fireEvent.click(
      within(llmGroup).getByRole("radio", { name: /aws bedrock/i }),
    );
    const advanced = openSettingsAdvancedControls(llmGroup);
    fireEvent.change(within(advanced).getByLabelText(/credential mode/i), {
      target: { value: "access_keys" },
    });
    fireEvent.change(
      await openCredentialInput(
        advanced,
        /access key id/i,
        /add aws keys|replace aws keys/i,
      ),
      {
        target: { value: "AKIA_BEDROCK" },
      },
    );
    fireEvent.change(
      within(advanced).getByLabelText(/secret access key/i, {
        selector: "input",
      }),
      { target: { value: "secret-bedrock" } },
    );
    fireEvent.change(
      within(advanced).getByLabelText(/session token/i, {
        selector: "input",
      }),
      { target: { value: "session-bedrock" } },
    );

    await act(async () => {
      fireEvent.click(
        within(llmGroup).getByRole("button", { name: /test connection/i }),
      );
    });

    expect(mockedInvoke).toHaveBeenCalledWith("test_aws_credentials", {
      region: "us-east-1",
      credentialSource: { type: "access_keys", access_key: "AKIA_BEDROCK" },
      secretAccessKey: "secret-bedrock",
      sessionToken: "session-bedrock",
    });
    expect(saveCredentialCalls()).toHaveLength(0);
  });

  it("renders each implemented ASR radio option from the provider registry", () => {
    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    const radios = within(asrGroup).getAllByRole("radio");
    // 7 ASR providers wired up in AsrProviderSettings.
    expect(radios.length).toBe(7);
    expect(
      within(asrGroup).getByRole("radio", { name: /openai realtime/i }),
    ).toBeInTheDocument();
  });

  it("saves OpenAI Realtime ASR with the openai_api_key credential slot", async () => {
    const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
      async () => {},
    );
    resetStore({ saveSettings });
    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;

    fireEvent.click(
      within(asrGroup).getByRole("radio", { name: /openai realtime/i }),
    );
    fireEvent.change(await openCredentialInput(asrGroup, /api key/i), {
      target: { value: "sk-realtime" },
    });
    fireEvent.change(within(asrGroup).getByLabelText(/^model$/i), {
      target: { value: "gpt-realtime-whisper" },
    });
    fireEvent.change(within(asrGroup).getByLabelText(/language code/i), {
      target: { value: "en" },
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
    });

    expect(mockedInvoke).toHaveBeenCalledWith("save_credential_cmd", {
      key: "openai_api_key",
      value: "sk-realtime",
    });
    expect(saveSettings).toHaveBeenCalledTimes(1);
    expect(saveSettings.mock.calls[0]?.[0].asr_provider).toEqual({
      type: "openai_realtime",
      api_key: "",
      model: "gpt-realtime-whisper",
      language: "en",
    });
  });

  // ── Cerebras OpenAI-compatible LLM provider ───────────────────────────
  describe("Cerebras LLM provider", () => {
    it("uses saved Cerebras key presence without plaintext loadback and saves as an API preset", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      resetStore({ saveSettings });
      mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
        if (cmd === "load_credential_presence_cmd") {
          return [
            {
              key: "cerebras_api_key",
              present: true,
              source: "credentials_yaml",
            },
          ];
        }
        if (cmd === "load_credential_cmd")
          failPlaintextCredentialLoadback(args);
        if (cmd === "get_provider_readiness_cmd") {
          return [
            {
              provider_id: "llm.cerebras",
              status: "ready",
              message: "Cerebras API key is valid (2 models)",
              stale: false,
              credential_epoch: 1,
              credentials: [{ key: "cerebras_api_key", present: true }],
              model_count: 2,
              model_catalog: [
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
              ],
              openrouter_models: [],
            },
          ];
        }
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "save_credential_cmd") return undefined;
        return undefined;
      });

      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /cerebras/i }));

      expect(
        await screen.findByText(/saved Cerebras key available/i),
      ).toBeInTheDocument();
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      expect(
        within(llmGroup).queryByLabelText(/cerebras api key/i, {
          selector: "input",
        }),
      ).not.toBeInTheDocument();
      expect(
        within(llmGroup).getByRole("button", { name: /replace key/i }),
      ).toBeInTheDocument();
      expect(within(llmGroup).getByLabelText(/endpoint url/i)).toHaveValue(
        "https://api.cerebras.ai/v1",
      );
      expect(within(llmGroup).getByRole("combobox", { name: /model/i })).toBe(
        screen.getByDisplayValue("gpt-oss-120b"),
      );

      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
      });

      expect(
        mockedInvoke.mock.calls.some(
          ([cmd, args]) =>
            cmd === "load_credential_cmd" &&
            (args as { key?: string } | undefined)?.key === "cerebras_api_key",
        ),
      ).toBe(false);
      expect(
        saveCredentialCalls().some(
          ([, args]) =>
            (args as { key?: string } | undefined)?.key === "cerebras_api_key",
        ),
      ).toBe(false);
      expect(saveSettings).toHaveBeenCalledTimes(1);
      const saved = saveSettings.mock.calls[0]?.[0];
      expect(saved.llm_provider).toEqual({
        type: "api",
        endpoint: "https://api.cerebras.ai/v1",
        api_key: "",
        model: "gpt-oss-120b",
      });
      expect(saved.llm_api_config).toMatchObject({
        endpoint: "https://api.cerebras.ai/v1",
        api_key: null,
        model: "gpt-oss-120b",
      });
    });

    it("uses a saved Cerebras key for test and model loading without plaintext loadback", async () => {
      const fixtureModels = [
        {
          id: "qwen-3-235b",
          display_name: "Qwen 3 235B",
          is_default: false,
        },
        {
          id: "gpt-oss-120b",
          display_name: "OpenAI GPT OSS 120B",
          is_default: true,
        },
      ];
      mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
        if (cmd === "load_credential_presence_cmd") {
          return [
            {
              key: "cerebras_api_key",
              present: true,
              source: "credentials_yaml",
            },
          ];
        }
        if (cmd === "load_credential_cmd")
          failPlaintextCredentialLoadback(args);
        if (cmd === "get_provider_readiness_cmd") return [];
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "test_cerebras_connection_cmd") {
          return "Cerebras API key is valid";
        }
        if (cmd === "list_cerebras_models_cmd") return fixtureModels;
        return undefined;
      });

      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /cerebras/i }));

      expect(
        await screen.findByText(/saved Cerebras key available/i),
      ).toBeInTheDocument();
      const llmGroup = settingsSectionForHeading(/LLM Provider/i);
      expect(
        within(llmGroup).queryByLabelText(/cerebras api key/i, {
          selector: "input",
        }),
      ).not.toBeInTheDocument();
      expect(
        within(llmGroup).getByRole("button", { name: /replace key/i }),
      ).toBeInTheDocument();

      await act(async () => {
        fireEvent.click(
          within(llmGroup).getByRole("button", { name: /test connection/i }),
        );
      });
      expect(mockedInvoke).toHaveBeenCalledWith(
        "test_cerebras_connection_cmd",
        {
          apiKey: null,
        },
      );
      expect(
        await within(llmGroup).findByText(/Cerebras API key is valid/i),
      ).toBeInTheDocument();

      await act(async () => {
        fireEvent.click(
          within(llmGroup).getByRole("button", { name: /load models/i }),
        );
      });
      expect(mockedInvoke).toHaveBeenCalledWith("list_cerebras_models_cmd", {
        apiKey: null,
      });

      const picker = within(llmGroup).getByRole("combobox", {
        name: /model/i,
      }) as HTMLInputElement;
      fireEvent.change(picker, { target: { value: "qwen" } });
      const qwenOption = within(llmGroup).getByRole("option", {
        name: /Qwen 3 235B \(qwen-3-235b\)/i,
      });
      fireEvent.mouseDown(qwenOption);
      expect(picker).toHaveValue("qwen-3-235b");
      expect(
        mockedInvoke.mock.calls.some(
          ([cmd, args]) =>
            cmd === "load_credential_cmd" &&
            (args as { key?: string } | undefined)?.key === "cerebras_api_key",
        ),
      ).toBe(false);
      expect(saveCredentialCalls()).toHaveLength(0);
    });

    it("passes a typed Cerebras key to test and model refresh without saving it", async () => {
      const fixtureModels = [
        {
          id: "llama-4-scout-17b-16e-instruct",
          display_name: "Llama 4 Scout",
          is_default: false,
        },
      ];
      mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
        if (cmd === "load_credential_cmd")
          failPlaintextCredentialLoadback(args);
        if (cmd === "load_credential_presence_cmd") return [];
        if (cmd === "get_provider_readiness_cmd") return [];
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "test_cerebras_connection_cmd") {
          return "Cerebras typed key is valid";
        }
        if (cmd === "list_cerebras_models_cmd") return fixtureModels;
        if (cmd === "save_credential_cmd") return undefined;
        return undefined;
      });

      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /cerebras/i }));
      const llmGroup = settingsSectionForHeading(/LLM Provider/i);
      const keyInput = await openCredentialInput(llmGroup, /cerebras api key/i);
      fireEvent.change(keyInput, {
        target: { value: "typed-cerebras-key" },
      });

      await act(async () => {
        fireEvent.click(
          within(llmGroup).getByRole("button", { name: /test connection/i }),
        );
      });
      expect(mockedInvoke).toHaveBeenCalledWith(
        "test_cerebras_connection_cmd",
        {
          apiKey: "typed-cerebras-key",
        },
      );

      await act(async () => {
        fireEvent.click(
          within(llmGroup).getByRole("button", { name: /load models/i }),
        );
      });
      expect(mockedInvoke).toHaveBeenCalledWith("list_cerebras_models_cmd", {
        apiKey: "typed-cerebras-key",
      });
      expect(saveCredentialCalls()).toHaveLength(0);
    });

    it("shows a visible Cerebras model refresh error", async () => {
      mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
        if (cmd === "load_credential_cmd")
          failPlaintextCredentialLoadback(args);
        if (cmd === "load_credential_presence_cmd") return [];
        if (cmd === "get_provider_readiness_cmd") return [];
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "list_cerebras_models_cmd") {
          throw new Error("Cerebras catalog unavailable");
        }
        return undefined;
      });

      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /cerebras/i }));
      const llmGroup = settingsSectionForHeading(/LLM Provider/i);
      fireEvent.change(
        await openCredentialInput(llmGroup, /cerebras api key/i),
        {
          target: { value: "typed-cerebras-key" },
        },
      );

      await act(async () => {
        fireEvent.click(
          within(llmGroup).getByRole("button", { name: /load models/i }),
        );
      });

      expect(
        await within(llmGroup).findByText(/Cerebras catalog unavailable/i),
      ).toBeInTheDocument();
      expect(saveCredentialCalls()).toHaveLength(0);
    });
  });

  // ── OpenRouter (plan A2 / ADR-0005) ───────────────────────────────────
  describe("OpenRouter LLM provider", () => {
    const openAdvancedControls = (scope: HTMLElement) => {
      const summary = within(scope).getByText(/advanced provider controls/i);
      const details = summary.closest("details") as HTMLDetailsElement;
      expect(details).toBeTruthy();
      expect(details.open).toBe(false);
      fireEvent.click(summary);
      expect(details.open).toBe(true);
      return details;
    };

    it("LLM provider radio group includes OpenRouter as a labeled option", () => {
      render(<SettingsPage />);
      goToTab(/language model/i);
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      // 6 LLM providers: local_llama, api, cerebras, openrouter, aws_bedrock, mistralrs.
      const radios = within(llmGroup).getAllByRole("radio");
      expect(radios.length).toBe(6);
      expect(
        within(llmGroup).getByRole("radio", { name: /cerebras/i }),
      ).toBeInTheDocument();
      expect(
        within(llmGroup).getByRole("radio", { name: /openrouter/i }),
      ).toBeInTheDocument();
    });

    it("selecting OpenRouter keeps normal setup visible and expert controls behind Advanced", () => {
      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      expect(
        within(llmGroup).queryByLabelText(/openrouter api key/i, {
          selector: "input",
        }),
      ).not.toBeInTheDocument();
      expect(
        within(llmGroup).getByRole("button", { name: /add key/i }),
      ).toBeInTheDocument();
      // The model select renders with the empty-state placeholder.
      expect(
        screen.getByRole("combobox", { name: /openrouter model/i }),
      ).toBeInTheDocument();
      expect(
        within(llmGroup).getByRole("button", { name: /load models/i }),
      ).toBeInTheDocument();
      expect(
        within(llmGroup).getByRole("button", { name: /test connection/i }),
      ).toBeInTheDocument();
      const advanced = openAdvancedControls(llmGroup);
      expect(
        within(advanced).getByLabelText(/endpoint url/i),
      ).toHaveDisplayValue("https://openrouter.ai/api/v1");
      expect(
        within(advanced).getByRole("checkbox", {
          name: /include token usage/i,
        }),
      ).toBeChecked();
      expect(
        within(advanced).getByLabelText(/openrouter routing preset/i),
      ).toHaveValue("balanced");
      expect(
        within(advanced).queryByLabelText(/accelerator\/provider slugs/i),
      ).not.toBeInTheDocument();
    });

    it("preserves legacy OpenRouter provider_order through hydrate and save", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      resetStore({
        saveSettings,
        settings: {
          ...baseSettings,
          llm_provider: {
            type: "openrouter",
            api_key: "",
            model: "openai/gpt-5.2",
            base_url: "https://openrouter.ai/api/v1",
            provider_order: ["anthropic", "openai"],
            include_usage_in_stream: true,
          },
          openrouter_routing_policy: null,
        },
      });

      render(<SettingsPage />);
      goToTab(/language model/i);
      const llmGroup = settingsSectionForHeading(/LLM Provider/i);
      await waitFor(() =>
        expect(
          within(llmGroup).getByRole("radio", { name: /openrouter/i }),
        ).toBeChecked(),
      );

      await clickSaveSettings();

      expect(saveSettings).toHaveBeenCalledTimes(1);
      const saved = saveSettings.mock.calls[0]?.[0];
      expect(saved.openrouter_routing_policy).toBeNull();
      expect(saved.llm_provider.type).toBe("openrouter");
      if (saved.llm_provider.type === "openrouter") {
        expect(saved.llm_provider.provider_order).toEqual([
          "anthropic",
          "openai",
        ]);
      }
    });

    it("serializes the strict accelerator routing preset with locked provider order", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      resetStore({ saveSettings });

      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = settingsSectionForHeading(/LLM Provider/i);
      const advanced = openAdvancedControls(llmGroup);

      fireEvent.change(
        within(advanced).getByLabelText(/openrouter routing preset/i),
        { target: { value: "strict_accelerator" } },
      );
      fireEvent.change(
        within(advanced).getByLabelText(/accelerator\/provider slugs/i),
        { target: { value: "cerebras\ngroq" } },
      );

      await clickSaveSettings();

      expect(saveSettings).toHaveBeenCalledTimes(1);
      const saved = saveSettings.mock.calls[0]?.[0];
      expect(saved.openrouter_routing_policy).toMatchObject({
        order: ["cerebras", "groq"],
        only: ["cerebras", "groq"],
        allow_fallbacks: false,
      });
      expect(saved.llm_provider.type).toBe("openrouter");
      if (saved.llm_provider.type === "openrouter") {
        expect(saved.llm_provider.provider_order).toBeNull();
      }
    });

    it("preserves custom rich OpenRouter routing policies through hydrate and save", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      const customRoutingPolicy: NonNullable<
        AppSettings["openrouter_routing_policy"]
      > = {
        order: ["openai"],
        only: [],
        ignore: ["novita"],
        quantizations: ["fp8"],
        sort: { by: "latency", partition: "model" },
        preferred_max_latency: { p50: 0.75, p90: 2.0 },
        data_collection: "deny",
        max_price: { prompt: 0.000005 },
      };
      resetStore({
        saveSettings,
        settings: {
          ...baseSettings,
          llm_provider: {
            type: "openrouter",
            api_key: "",
            model: "openai/gpt-5.2",
            base_url: "https://openrouter.ai/api/v1",
            provider_order: null,
            include_usage_in_stream: true,
          },
          openrouter_routing_policy: customRoutingPolicy,
        },
      });

      render(<SettingsPage />);
      goToTab(/language model/i);
      const llmGroup = settingsSectionForHeading(/LLM Provider/i);
      await waitFor(() =>
        expect(
          within(llmGroup).getByRole("radio", { name: /openrouter/i }),
        ).toBeChecked(),
      );
      const advanced = openAdvancedControls(llmGroup);
      expect(
        within(advanced).getByLabelText(/openrouter routing preset/i),
      ).toHaveValue("custom");

      await clickSaveSettings();

      expect(saveSettings).toHaveBeenCalledTimes(1);
      const saved = saveSettings.mock.calls[0]?.[0];
      expect(saved.openrouter_routing_policy).toEqual(customRoutingPolicy);
      expect(saveCredentialCalls()).toHaveLength(0);
    });

    it("serializes the low-latency OpenRouter routing preset with second-based latency targets", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      resetStore({ saveSettings });

      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = settingsSectionForHeading(/LLM Provider/i);
      const advanced = openAdvancedControls(llmGroup);

      fireEvent.change(
        within(advanced).getByLabelText(/openrouter routing preset/i),
        { target: { value: "low_latency" } },
      );

      await clickSaveSettings();

      expect(saveSettings).toHaveBeenCalledTimes(1);
      const saved = saveSettings.mock.calls[0]?.[0];
      expect(saved.openrouter_routing_policy).toEqual({
        order: [],
        only: [],
        ignore: [],
        quantizations: [],
        sort: { by: "latency", partition: "model" },
        preferred_max_latency: { p50: 0.75, p90: 2.0 },
      });
    });

    it("serializes the high-throughput Nitro OpenRouter routing preset", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      resetStore({ saveSettings });

      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = settingsSectionForHeading(/LLM Provider/i);
      const advanced = openAdvancedControls(llmGroup);

      fireEvent.change(
        within(advanced).getByLabelText(/openrouter routing preset/i),
        { target: { value: "high_throughput" } },
      );

      await clickSaveSettings();

      expect(saveSettings).toHaveBeenCalledTimes(1);
      const saved = saveSettings.mock.calls[0]?.[0];
      expect(saved.openrouter_routing_policy).toEqual({
        order: [],
        only: [],
        ignore: [],
        quantizations: [],
        sort: { by: "throughput", partition: "model" },
        preferred_min_throughput: { p50: 40, p90: 20 },
      });
    });

    it("serializes the privacy ZDR routing preset as data_collection deny with zdr", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      resetStore({ saveSettings });

      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = settingsSectionForHeading(/LLM Provider/i);
      const advanced = openAdvancedControls(llmGroup);

      fireEvent.change(
        within(advanced).getByLabelText(/openrouter routing preset/i),
        { target: { value: "privacy_zdr" } },
      );

      await clickSaveSettings();

      expect(saveSettings).toHaveBeenCalledTimes(1);
      const saved = saveSettings.mock.calls[0]?.[0];
      expect(saved.openrouter_routing_policy).toMatchObject({
        data_collection: "deny",
        zdr: true,
      });
    });

    it("keeps OpenRouter expert controls behind Advanced and still saves them", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      resetStore({ saveSettings });
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "save_credential_cmd") return undefined;
        return undefined;
      });
      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      const advanced = openAdvancedControls(llmGroup);

      fireEvent.change(within(advanced).getByLabelText(/endpoint url/i), {
        target: { value: "https://openrouter.example/api/v1/" },
      });
      fireEvent.click(
        within(advanced).getByRole("checkbox", {
          name: /include token usage/i,
        }),
      );
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
      });

      expect(saveSettings).toHaveBeenCalledTimes(1);
      const arg = saveSettings.mock.calls[0]?.[0];
      expect(arg.llm_provider.type).toBe("openrouter");
      if (arg.llm_provider.type === "openrouter") {
        expect(arg.llm_provider.base_url).toBe(
          "https://openrouter.example/api/v1",
        );
        expect(arg.llm_provider.include_usage_in_stream).toBe(false);
      }
    });

    it("uses a saved OpenRouter key for test and model loading without hydrating it into the field", async () => {
      const fixtureModels = [
        {
          id: "anthropic/claude-sonnet-4.5",
          name: "Anthropic: Claude Sonnet 4.5",
          context_length: 200000,
          pricing: { prompt: "0.000003", completion: "0.000015" },
        },
      ];
      mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
        if (cmd === "load_credential_presence_cmd") {
          return [
            {
              key: "openrouter_api_key",
              present: true,
              source: "credentials_yaml",
            },
          ];
        }
        if (cmd === "load_credential_cmd")
          failPlaintextCredentialLoadback(args);
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "test_openrouter_connection_cmd") {
          return "OpenRouter API key is valid";
        }
        if (cmd === "list_openrouter_models_cmd") return fixtureModels;
        return undefined;
      });

      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));

      expect(
        await screen.findByText(/saved OpenRouter key available/i),
      ).toBeInTheDocument();
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      expect(
        within(llmGroup).queryByLabelText(/openrouter api key/i, {
          selector: "input",
        }),
      ).not.toBeInTheDocument();
      expect(
        within(llmGroup).getByRole("button", { name: /replace key/i }),
      ).toBeInTheDocument();

      await act(async () => {
        fireEvent.click(
          within(llmGroup).getByRole("button", {
            name: /test connection/i,
          }),
        );
      });
      expect(mockedInvoke).toHaveBeenCalledWith(
        "test_openrouter_connection_cmd",
        {
          apiKey: null,
          baseUrl: "https://openrouter.ai/api/v1",
        },
      );

      await act(async () => {
        fireEvent.click(
          within(llmGroup).getByRole("button", { name: /load models/i }),
        );
      });
      expect(mockedInvoke).toHaveBeenCalledWith("list_openrouter_models_cmd", {
        apiKey: null,
        baseUrl: "https://openrouter.ai/api/v1",
      });
      expect(
        mockedInvoke.mock.calls.some(
          ([cmd, args]) =>
            cmd === "load_credential_cmd" &&
            (args as { key?: string } | undefined)?.key ===
              "openrouter_api_key",
        ),
      ).toBe(false);
    });

    it("clicking Test invokes test_openrouter_connection_cmd with the API key and base URL", async () => {
      // Pre-stage the invoke mock so the test command resolves.
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "save_credential_cmd") return undefined;
        if (cmd === "test_openrouter_connection_cmd") {
          return "OpenRouter API key is valid";
        }
        return undefined;
      });
      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      // Type the key.
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      const keyInput = await openCredentialInput(
        llmGroup,
        /openrouter api key/i,
      );
      fireEvent.change(keyInput, {
        target: { value: "sk-or-test-key" },
      });
      // Click the Test Connection button. Multiple Test buttons may
      // exist if other branches stayed mounted; pick the one inside
      // the LLM section.
      const testBtn = within(llmGroup).getByRole("button", {
        name: /test connection/i,
      });
      const advanced = openAdvancedControls(llmGroup);
      fireEvent.change(within(advanced).getByLabelText(/endpoint url/i), {
        target: { value: "https://openrouter.example/api/v1/" },
      });
      await act(async () => {
        fireEvent.click(testBtn);
      });
      expect(mockedInvoke).toHaveBeenCalledWith(
        "test_openrouter_connection_cmd",
        {
          apiKey: "sk-or-test-key",
          baseUrl: "https://openrouter.example/api/v1",
        },
      );
      expect(saveCredentialCalls()).toHaveLength(0);
    });

    it("clicking Load models invokes list_openrouter_models_cmd and populates the picker", async () => {
      const fixtureModels = [
        {
          id: "anthropic/claude-sonnet-4.5",
          name: "Anthropic: Claude Sonnet 4.5",
          context_length: 200000,
          pricing: { prompt: "0.000003", completion: "0.000015" },
        },
        {
          id: "openai/gpt-5.2",
          name: "OpenAI: GPT-5.2",
          context_length: 400000,
          pricing: { prompt: "0.000005", completion: "0.0000125" },
        },
      ];
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "save_credential_cmd") return undefined;
        if (cmd === "list_openrouter_models_cmd") return fixtureModels;
        return undefined;
      });
      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      const keyInput = await openCredentialInput(
        llmGroup,
        /openrouter api key/i,
      );
      fireEvent.change(keyInput, {
        target: { value: "sk-or-test-key" },
      });
      const loadBtn = within(llmGroup).getByRole("button", {
        name: /load models/i,
      });
      await act(async () => {
        fireEvent.click(loadBtn);
      });
      expect(mockedInvoke).toHaveBeenCalledWith("list_openrouter_models_cmd", {
        apiKey: "sk-or-test-key",
        baseUrl: "https://openrouter.ai/api/v1",
      });
      expect(saveCredentialCalls()).toHaveLength(0);
      // The shared picker exposes fixture options through its searchable listbox.
      const picker = within(llmGroup).getByRole("combobox", {
        name: /openrouter model/i,
      }) as HTMLInputElement;
      fireEvent.change(picker, { target: { value: "gpt" } });
      expect(
        within(llmGroup).getByRole("option", {
          name: /OpenAI: GPT-5\.2 \(openai\/gpt-5\.2\)/i,
        }),
      ).toBeInTheDocument();
      fireEvent.change(picker, { target: { value: "claude" } });
      expect(
        within(llmGroup).getByRole("option", {
          name: /Anthropic: Claude Sonnet 4\.5 \(anthropic\/claude-sonnet-4\.5\)/i,
        }),
      ).toBeInTheDocument();
    });

    it("saves a custom OpenRouter model when the catalog returns empty", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      resetStore({ saveSettings });
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "save_credential_cmd") return undefined;
        if (cmd === "list_openrouter_models_cmd") return [];
        return undefined;
      });
      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      fireEvent.change(
        await openCredentialInput(llmGroup, /openrouter api key/i),
        {
          target: { value: "sk-or-test-key" },
        },
      );

      await act(async () => {
        fireEvent.click(
          within(llmGroup).getByRole("button", { name: /load models/i }),
        );
      });

      expect(mockedInvoke).toHaveBeenCalledWith("list_openrouter_models_cmd", {
        apiKey: "sk-or-test-key",
        baseUrl: "https://openrouter.ai/api/v1",
      });
      const picker = within(llmGroup).getByRole("combobox", {
        name: /openrouter model/i,
      }) as HTMLInputElement;
      expect(
        within(llmGroup).getByText(
          /No catalog loaded\. Type a custom model id/i,
        ),
      ).toBeInTheDocument();

      fireEvent.change(picker, {
        target: { value: "custom/openrouter-fallback" },
      });
      expect(picker).toHaveValue("custom/openrouter-fallback");

      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
      });

      expect(saveSettings).toHaveBeenCalledTimes(1);
      const saved = saveSettings.mock.calls[0]?.[0];
      expect(saved.llm_provider.type).toBe("openrouter");
      if (saved.llm_provider.type === "openrouter") {
        expect(saved.llm_provider.model).toBe("custom/openrouter-fallback");
      }
      expect(saveCredentialCalls()).toHaveLength(1);
    });

    it("shows a visible OpenRouter model refresh error when loading models rejects", async () => {
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "save_credential_cmd") return undefined;
        if (cmd === "list_openrouter_models_cmd") {
          throw new Error("OpenRouter catalog unavailable");
        }
        return undefined;
      });
      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      fireEvent.change(
        await openCredentialInput(llmGroup, /openrouter api key/i),
        {
          target: { value: "sk-or-test-key" },
        },
      );

      await act(async () => {
        fireEvent.click(
          within(llmGroup).getByRole("button", { name: /load models/i }),
        );
      });

      expect(
        await within(llmGroup).findByText(/OpenRouter catalog unavailable/i),
      ).toBeInTheDocument();
    });

    it("changing the OpenRouter base URL bypasses the model catalog cache", async () => {
      const fixtureModels = [
        {
          id: "custom/model",
          name: "Custom Model",
          context_length: 32000,
          pricing: { prompt: "0", completion: "0" },
        },
      ];
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "list_openrouter_models_cmd") return fixtureModels;
        return undefined;
      });

      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      const keyInput = await openCredentialInput(
        llmGroup,
        /openrouter api key/i,
      );
      fireEvent.change(keyInput, {
        target: { value: "sk-or-test-key" },
      });
      const loadBtn = within(llmGroup).getByRole("button", {
        name: /load models/i,
      });

      await act(async () => {
        fireEvent.click(loadBtn);
      });
      const advanced = openAdvancedControls(llmGroup);
      fireEvent.change(within(advanced).getByLabelText(/endpoint url/i), {
        target: { value: "https://openrouter.local/api/v1/" },
      });
      await act(async () => {
        fireEvent.click(loadBtn);
      });

      const listCalls = mockedInvoke.mock.calls.filter(
        ([cmd]) => cmd === "list_openrouter_models_cmd",
      );
      expect(listCalls).toHaveLength(2);
      expect(listCalls[0]).toEqual([
        "list_openrouter_models_cmd",
        {
          apiKey: "sk-or-test-key",
          baseUrl: "https://openrouter.ai/api/v1",
        },
      ]);
      expect(listCalls[1]).toEqual([
        "list_openrouter_models_cmd",
        {
          apiKey: "sk-or-test-key",
          baseUrl: "https://openrouter.local/api/v1",
        },
      ]);
      expect(saveCredentialCalls()).toHaveLength(0);
    });

    it("Save persists the OpenRouter provider with the chosen model", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      resetStore({ saveSettings });
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "save_credential_cmd") return undefined;
        return undefined;
      });
      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      const keyInput = await openCredentialInput(
        llmGroup,
        /openrouter api key/i,
      );
      fireEvent.change(keyInput, {
        target: { value: "sk-or-some-key" },
      });
      // Force an option into the picker by directly dispatching through
      // settingsReducer would require store internals — easier: type
      // a model id via a fixture catalog populated by the previous
      // step. We simulate by also dispatching openrouterModel via the
      // select change. Add an option dynamically by going through the
      // load-models flow.
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "save_credential_cmd") return undefined;
        if (cmd === "list_openrouter_models_cmd") {
          return [
            {
              id: "openai/gpt-5.2",
              name: "OpenAI: GPT-5.2",
              context_length: 400000,
              pricing: {
                prompt: "0.000005",
                completion: "0.0000125",
              },
            },
          ];
        }
        return undefined;
      });
      const loadBtn = within(llmGroup).getByRole("button", {
        name: /load models/i,
      });
      await act(async () => {
        fireEvent.click(loadBtn);
      });
      const picker = within(llmGroup).getByRole("combobox", {
        name: /openrouter model/i,
      }) as HTMLInputElement;
      fireEvent.change(picker, {
        target: { value: "gpt" },
      });
      fireEvent.keyDown(picker, { key: "Enter" });
      expect(picker.value).toBe("openai/gpt-5.2");
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: /save settings/i }));
      });
      expect(saveSettings).toHaveBeenCalledTimes(1);
      const arg = saveSettings.mock.calls[0]?.[0];
      expect(arg.llm_provider.type).toBe("openrouter");
      if (arg.llm_provider.type === "openrouter") {
        expect(arg.llm_provider.model).toBe("openai/gpt-5.2");
        expect(arg.llm_provider.base_url).toBe("https://openrouter.ai/api/v1");
        expect(arg.llm_provider.include_usage_in_stream).toBe(true);
      }
    });
  });
});
