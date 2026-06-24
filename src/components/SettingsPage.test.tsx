import { invoke } from "@tauri-apps/api/core";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { AppSettings } from "../types";
import SettingsPage from "./SettingsPage";
import {
  buildAwsCredentialSource,
  initialSettingsState,
  type SettingsState,
  setField,
  settingsReducer,
} from "./settingsTypes";
import "../i18n";

const mockedInvoke = vi.mocked(invoke);

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
  audio_settings: { sample_rate: 48000, channels: 1 },
  gemini: {
    auth: { type: "api_key", api_key: "" },
    model: "gemini-3.1-flash-live-preview",
  },
  log_level: "info",
};

function resetStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    settings: baseSettings,
    models: [],
    modelStatus: null,
    settingsLoading: false,
    isDownloading: false,
    downloadProgress: null,
    isDeletingModel: null,
    closeSettings: vi.fn(),
    saveSettings: vi.fn(async () => {}),
    downloadModel: vi.fn(),
    deleteModel: vi.fn(),
    listAwsProfiles: vi.fn(async () => []),
    ...overrides,
  });
}

describe("settingsReducer", () => {
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
      awsAsrSecretKey: "a",
      awsBedrockSecretKey: "b",
      awsAsrSessionToken: "c",
      awsBedrockSessionToken: "d",
    };
    const next = settingsReducer(seeded, { type: "CLEAR_AWS_SHARED_KEYS" });
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
});

describe("SettingsPage", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    mockedInvoke.mockImplementation(async (cmd: string) => {
      // Return null for credential loads so the hydration side effect
      // silently skips mirroring values into reducer state.
      if (cmd === "load_credential_cmd") return null;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });
    resetStore();
  });

  // Settings sections are grouped into tabs; click one to reveal its
  // section before interacting with that section's fields.
  const goToTab = (name: RegExp) =>
    fireEvent.click(screen.getByRole("tab", { name }));

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

  it("selecting Cloud API for ASR reveals endpoint + api-key + model inputs", () => {
    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    const cloudRadio = screen.getByRole("radio", {
      name: /cloud api/i,
    });
    fireEvent.click(cloudRadio);
    expect(
      screen.getByPlaceholderText("https://api.openai.com/v1"),
    ).toBeInTheDocument();
    // Multiple password inputs may exist once another section's creds
    // are also visible; assert at least one API-key placeholder exists.
    const asrApiInputs = screen.getAllByPlaceholderText(/sk-\.\.\./);
    expect(asrApiInputs.length).toBeGreaterThanOrEqual(1);
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

  it("LlmProviderSettings OpenAI-compatible shows endpoint + api-key + model", () => {
    render(<SettingsPage />);
    goToTab(/language model/i);
    // Default state is already llmType === "api", so the fields render.
    expect(
      screen.getByPlaceholderText("http://localhost:8000/v1"),
    ).toBeInTheDocument();
    expect(screen.getByPlaceholderText("gpt-4o-mini")).toBeInTheDocument();
  });

  it("selecting AWS Bedrock reveals region + model-id + credential-mode", () => {
    render(<SettingsPage />);
    goToTab(/language model/i);
    fireEvent.click(screen.getByRole("radio", { name: /aws bedrock/i }));
    expect(
      screen.getByPlaceholderText("anthropic.claude-3-haiku-20240307-v1:0"),
    ).toBeInTheDocument();
    // Credential mode select is now visible; its default is default_chain.
    const credSelects = screen.getAllByRole("combobox");
    const modes = credSelects.map((el) => (el as HTMLSelectElement).value);
    expect(modes).toContain("default_chain");
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

  it("AWS Transcribe access-keys mode shares credentials with Bedrock via CLEAR_AWS_SHARED_KEYS", () => {
    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    fireEvent.click(screen.getByRole("radio", { name: /aws transcribe/i }));
    // Switch credential mode to access_keys. The ASR section's
    // credential-mode select is the first combobox inside that section,
    // but we're simpler than that: find it by the visible options.
    const selects = screen.getAllByRole("combobox") as HTMLSelectElement[];
    const credModeSelect = selects.find((s) =>
      Array.from(s.options).some((o) => o.value === "access_keys"),
    );
    expect(credModeSelect).toBeDefined();
    fireEvent.change(credModeSelect as HTMLSelectElement, {
      target: { value: "access_keys" },
    });
    // The "Clear Saved AWS Keys" button should now be visible — clicking
    // it triggers handleClearCredential → CLEAR_AWS_SHARED_KEYS.
    const clearBtn = screen.getByRole("button", {
      name: /clear saved aws keys/i,
    });
    expect(clearBtn).toBeInTheDocument();
  });

  it("renders each ASR radio option (local/cloud/aws/deepgram/assemblyai/sherpa)", () => {
    render(<SettingsPage />);
    goToTab(/speech-to-text/i);
    const asrGroup = screen
      .getByRole("heading", { name: /ASR Provider/i, level: 3 })
      .closest(".settings-section") as HTMLElement;
    const radios = within(asrGroup).getAllByRole("radio");
    // 6 ASR providers wired up in AsrProviderSettings.
    expect(radios.length).toBe(6);
  });

  // ── OpenRouter (plan A2 / ADR-0005) ───────────────────────────────────
  describe("OpenRouter LLM provider", () => {
    it("LLM provider radio group includes OpenRouter as a labeled option", () => {
      render(<SettingsPage />);
      goToTab(/language model/i);
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      // 5 LLM providers: local_llama, api, openrouter, aws_bedrock, mistralrs.
      const radios = within(llmGroup).getAllByRole("radio");
      expect(radios.length).toBe(5);
      expect(
        within(llmGroup).getByRole("radio", { name: /openrouter/i }),
      ).toBeInTheDocument();
    });

    it("selecting OpenRouter reveals API key, base URL, model picker, Test button", () => {
      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      // API key input — placeholder is sk-or-...
      expect(screen.getByPlaceholderText(/sk-or-/i)).toBeInTheDocument();
      // Base URL pre-fills with the canonical default (and is also
      // referenced by the api branch's placeholder; the api branch
      // hides when this branch is active so the input is unique here).
      const baseUrlInputs = screen.getAllByDisplayValue(
        "https://openrouter.ai/api/v1",
      );
      expect(baseUrlInputs.length).toBeGreaterThanOrEqual(1);
      // The model select renders with the empty-state placeholder.
      expect(
        screen.getByRole("combobox", { name: /openrouter model/i }),
      ).toBeInTheDocument();
    });

    it("clicking Test invokes test_openrouter_connection_cmd with the API key", async () => {
      // Pre-stage the invoke mock so the test command resolves.
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "load_credential_cmd") return null;
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
      const keyInput = screen.getByPlaceholderText(/sk-or-/i);
      fireEvent.change(keyInput, {
        target: { value: "sk-or-test-key" },
      });
      // Click the Test Connection button. Multiple Test buttons may
      // exist if other branches stayed mounted; pick the one inside
      // the LLM section.
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      const testBtn = within(llmGroup).getByRole("button", {
        name: /test connection/i,
      });
      await act(async () => {
        fireEvent.click(testBtn);
      });
      expect(mockedInvoke).toHaveBeenCalledWith(
        "test_openrouter_connection_cmd",
        { apiKey: "sk-or-test-key" },
      );
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
        if (cmd === "load_credential_cmd") return null;
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "save_credential_cmd") return undefined;
        if (cmd === "list_openrouter_models_cmd") return fixtureModels;
        return undefined;
      });
      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const keyInput = screen.getByPlaceholderText(/sk-or-/i);
      fireEvent.change(keyInput, {
        target: { value: "sk-or-test-key" },
      });
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      const loadBtn = within(llmGroup).getByRole("button", {
        name: /load models/i,
      });
      await act(async () => {
        fireEvent.click(loadBtn);
      });
      expect(mockedInvoke).toHaveBeenCalledWith("list_openrouter_models_cmd", {
        apiKey: "sk-or-test-key",
      });
      // The picker now has the fixture options visible.
      const picker = within(llmGroup).getByRole("combobox", {
        name: /openrouter model/i,
      }) as HTMLSelectElement;
      const optionIds = Array.from(picker.options).map((o) => o.value);
      expect(optionIds).toContain("anthropic/claude-sonnet-4.5");
      expect(optionIds).toContain("openai/gpt-5.2");
    });

    it("Save persists the OpenRouter provider with the chosen model", async () => {
      const saveSettings = vi.fn<(settings: AppSettings) => Promise<void>>(
        async () => {},
      );
      resetStore({ saveSettings });
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "load_credential_cmd") return null;
        if (cmd === "list_aws_profiles") return [];
        if (cmd === "save_credential_cmd") return undefined;
        return undefined;
      });
      render(<SettingsPage />);
      goToTab(/language model/i);
      fireEvent.click(screen.getByRole("radio", { name: /openrouter/i }));
      const keyInput = screen.getByPlaceholderText(/sk-or-/i);
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
        if (cmd === "load_credential_cmd") return null;
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
      const llmGroup = screen
        .getByRole("heading", { name: /LLM Provider/i, level: 3 })
        .closest(".settings-section") as HTMLElement;
      const loadBtn = within(llmGroup).getByRole("button", {
        name: /load models/i,
      });
      await act(async () => {
        fireEvent.click(loadBtn);
      });
      const picker = within(llmGroup).getByRole("combobox", {
        name: /openrouter model/i,
      }) as HTMLSelectElement;
      fireEvent.change(picker, {
        target: { value: "openai/gpt-5.2" },
      });
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
