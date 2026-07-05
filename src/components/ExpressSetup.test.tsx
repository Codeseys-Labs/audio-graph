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
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import type {
  AppSettings,
  AudioSourceInfo,
  CredentialPresence,
  ProviderReadiness,
  ProviderRuntimeReadiness,
} from "../types";
import ExpressSetup from "./ExpressSetup";

const mockedInvoke = vi.mocked(invoke);
const GEMINI_OPENAI_ENDPOINT =
  "https://generativelanguage.googleapis.com/v1beta/openai";

const savedSettingsArg = (): AppSettings => {
  const saveSettings = mockedInvoke.mock.calls.find(
    ([cmd]) => cmd === "save_settings_cmd",
  );
  expect(saveSettings).toBeTruthy();
  return (saveSettings?.[1] as { settings: AppSettings }).settings;
};

const savedCredentialKeys = (): string[] =>
  mockedInvoke.mock.calls
    .filter(([cmd]) => cmd === "save_credential_cmd")
    .map(([, args]) => (args as { key: string }).key);

const makeExistingSettings = (): AppSettings => ({
  asr_provider: { type: "local_whisper" },
  whisper_model: "ggml-small.en.bin",
  llm_provider: { type: "local_llama" },
  llm_api_config: null,
  audio_settings: {
    sample_rate: 44100,
    channels: 1,
  },
  gemini: {
    auth: {
      type: "vertex_ai",
      project_id: "audio-graph-prod",
      location: "us-central1",
      service_account_path: "/config/gemini-service-account.json",
    },
    model: "gemini-live-existing",
    voice: "Kore",
  },
  tts_provider: { type: "none" },
  speak_aloud: false,
  streaming_prefill: true,
  log_level: "debug",
  demo_mode: true,
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

const credentialPresence = (...keys: string[]): CredentialPresence[] =>
  keys.map((key) => ({
    key,
    present: true,
    source: "credentials_yaml",
  }));

const readyProvider = (
  providerId: string,
  credentials: readonly string[] = [],
  runtime?: ProviderRuntimeReadiness,
): ProviderReadiness => ({
  provider_id: providerId,
  status: "ready",
  message: `${providerId} ready`,
  stale: false,
  credential_epoch: 1,
  credentials: credentials.map((key) => ({ key, present: true })),
  model_catalog: [],
  runtime: runtime ?? null,
});

function failPlaintextCredentialLoadback(args?: unknown): never {
  void args;
  throw new Error(
    "load_credential_cmd should not be invoked by frontend tests; use load_credential_presence_cmd and provider readiness instead.",
  );
}

const mockProviderState = ({
  presence = [],
  readiness = [],
}: {
  presence?: CredentialPresence[];
  readiness?: ProviderReadiness[];
}) => {
  mockedInvoke.mockImplementation(async (cmd: string) => {
    if (cmd === "load_credential_presence_cmd") return presence;
    if (cmd === "get_provider_readiness_cmd") return readiness;
    if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
    return undefined;
  });
};

describe("ExpressSetup", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useAudioGraphStore.setState({
      samplePreviewActive: false,
      settings: null,
      audioSources: [selectedSystemSource()],
      selectedSourceIds: ["system-default"],
      sourceRecoveryIntent: null,
      nativeS2sEnabled: false,
      conversationMode: "notes",
      converseEngine: "pipelined",
    });
    // Default: any save_* command succeeds; saved-key state comes from
    // credential presence/readiness, never plaintext credential loadback.
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") return [];
      if (cmd === "get_provider_readiness_cmd") return [];
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
      return undefined;
    });
  });

  it("renders the quickstart dialog with ASR and LLM provider selectors", () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);
    expect(
      screen.getByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
    // ASR and LLM dropdowns are present and default to a cloud provider
    // so the API-key field is visible.
    expect(
      screen.getByLabelText(/ASR \(speech-to-text\) provider/i),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(/LLM \(chat\) provider/i)).toBeInTheDocument();
    expect(
      screen.getByRole("option", {
        name: "Gemini ASR (cloud speech-to-text)",
      }),
    ).toHaveValue("gemini");
    expect(
      screen.getByLabelText(/configure native gemini live realtime mode/i),
    ).toBeInTheDocument();
    // Both cloud providers need a key → there are two API key inputs.
    expect(screen.getAllByLabelText(/API key/i)).toHaveLength(2);
    expect(
      screen.getAllByText(
        /add a key here to make the selected provider path runnable/i,
      ),
    ).toHaveLength(2);
  });

  it("hides the API key input when Local Whisper is selected for ASR", () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);
    const asrSelect = screen.getByLabelText(
      /ASR \(speech-to-text\) provider/i,
    ) as HTMLSelectElement;
    fireEvent.change(asrSelect, { target: { value: "local_whisper" } });
    // Now only the LLM (default OpenAI, still cloud) shows a key input.
    expect(screen.getAllByLabelText(/API key/i)).toHaveLength(1);
  });

  it("disables Save setup until required cloud keys are filled", () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);
    const save = screen.getByRole("button", { name: /save setup/i });
    expect(save).toBeDisabled();

    // Fill ASR key (Gemini by default).
    const asrKey = screen.getByLabelText(/ASR API key/i);
    fireEvent.change(asrKey, { target: { value: "gemini-key-123" } });
    expect(save).toBeDisabled(); // LLM still missing.

    const llmKey = screen.getByLabelText(/LLM API key/i);
    fireEvent.change(llmKey, { target: { value: "sk-openai-abc" } });
    expect(save).toBeEnabled();
  });

  it("uses saved OpenRouter presence for the hybrid card without prompting for plaintext", async () => {
    mockProviderState({
      presence: credentialPresence("openrouter_api_key"),
      readiness: [
        readyProvider("asr.local_whisper", [], {
          status: "healthy",
          message: "whisper model ready",
          model_id: "ggml-small.en.bin",
        }),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
    });
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    fireEvent.change(
      screen.getByLabelText(
        /ASR \(speech-to-text\) provider/i,
      ) as HTMLSelectElement,
      { target: { value: "local_whisper" } },
    );
    fireEvent.change(
      screen.getByLabelText(/LLM \(chat\) provider/i) as HTMLSelectElement,
      { target: { value: "openrouter" } },
    );

    await waitFor(() =>
      expect(screen.queryByLabelText(/LLM API key/i)).not.toBeInTheDocument(),
    );
    expect(screen.queryByLabelText(/ASR API key/i)).not.toBeInTheDocument();
    expect(
      screen.getByText(/backend will use it without re-entry/i),
    ).toBeInTheDocument();

    const hybridCard = screen.getByTestId("express-mode-card-hybrid");
    expect(hybridCard).toHaveTextContent(/Hybrid \(selected\)/i);
    expect(hybridCard).toHaveTextContent(/OpenRouter/i);
    expect(hybridCard).toHaveTextContent(/openrouter_api_key: present/i);
    expect(screen.getByRole("button", { name: /save setup/i })).toBeEnabled();
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("uses saved Deepgram presence while keeping the missing LLM blocker visible", async () => {
    mockProviderState({
      presence: credentialPresence("deepgram_api_key"),
      readiness: [readyProvider("asr.deepgram", ["deepgram_api_key"])],
    });
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    fireEvent.change(
      screen.getByLabelText(
        /ASR \(speech-to-text\) provider/i,
      ) as HTMLSelectElement,
      { target: { value: "deepgram" } },
    );

    await waitFor(() =>
      expect(screen.queryByLabelText(/ASR API key/i)).not.toBeInTheDocument(),
    );
    expect(
      screen.getByText(/backend will use it without re-entry/i),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(/LLM API key/i)).toBeInTheDocument();
    expect(
      screen.getByText(
        /add a key here to make the selected provider path runnable/i,
      ),
    ).toBeInTheDocument();

    const cloudCard = screen.getByTestId("express-mode-card-cloud_fast");
    expect(cloudCard).toHaveTextContent(/Cloud fast \(selected\)/i);
    expect(cloudCard).toHaveTextContent(/Deepgram streaming/i);
    expect(cloudCard).toHaveTextContent(/deepgram_api_key: present/i);
    expect(cloudCard).toHaveTextContent(/openai_api_key: missing/i);
    expect(screen.getByRole("button", { name: /save setup/i })).toBeDisabled();
  });

  it("uses saved Gemini Live readiness without a separate key prompt or secret readback", async () => {
    mockProviderState({
      presence: credentialPresence("gemini_api_key", "openai_api_key"),
      readiness: [
        readyProvider("asr.api", ["gemini_api_key"]),
        readyProvider("llm.api", ["openai_api_key"]),
        readyProvider("realtime_agent.gemini_live", ["gemini_api_key"]),
      ],
    });
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    await waitFor(() =>
      expect(screen.queryByLabelText(/ASR API key/i)).not.toBeInTheDocument(),
    );
    expect(screen.queryByLabelText(/LLM API key/i)).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByLabelText(/configure native gemini live realtime mode/i),
    );

    expect(
      screen.queryByLabelText(/Gemini Live API key/i),
    ).not.toBeInTheDocument();
    expect(
      screen.getAllByText(/backend will use it without re-entry/i).length,
    ).toBeGreaterThanOrEqual(1);
    const nativeCard = screen.getByTestId("express-mode-card-native_realtime");
    expect(nativeCard).toHaveTextContent(/Native realtime \(selected\)/i);
    expect(nativeCard).toHaveTextContent(/Gemini Live/i);
    expect(nativeCard).toHaveTextContent(/gemini_api_key: present/i);
    expect(screen.getByRole("button", { name: /save setup/i })).toBeEnabled();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save setup/i }));
    });

    expect(savedCredentialKeys()).toEqual([]);
    expect(savedSettingsArg().gemini.auth).toEqual({
      type: "api_key",
      api_key: "",
    });
    expect(useAudioGraphStore.getState().conversationMode).toBe("converse");
    expect(useAudioGraphStore.getState().converseEngine).toBe("native");
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
  });

  it("keeps native realtime unselected when only the legacy flag is stale", async () => {
    useAudioGraphStore.setState({
      nativeS2sEnabled: true,
      conversationMode: "notes",
      converseEngine: "native",
    });
    mockProviderState({
      presence: credentialPresence("gemini_api_key", "openai_api_key"),
      readiness: [
        readyProvider("asr.api", ["gemini_api_key"]),
        readyProvider("llm.api", ["openai_api_key"]),
        readyProvider("realtime_agent.gemini_live", ["gemini_api_key"]),
      ],
    });

    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    await waitFor(() =>
      expect(screen.queryByLabelText(/ASR API key/i)).not.toBeInTheDocument(),
    );
    const cloudCard = screen.getByTestId("express-mode-card-cloud_fast");
    expect(cloudCard).toHaveTextContent(/Cloud fast \(selected\)/i);
    const nativeCard = screen.getByTestId("express-mode-card-native_realtime");
    expect(nativeCard).not.toHaveTextContent(/Native realtime \(selected\)/i);
  });

  it("shows no-key blockers from provider setup mode cards", () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    const cloudCard = screen.getByTestId("express-mode-card-cloud_fast");
    expect(cloudCard).toHaveTextContent(/Cloud fast \(selected\)/i);
    expect(cloudCard).toHaveTextContent(/Missing credentials/i);
    expect(cloudCard).toHaveTextContent(/gemini_api_key: missing/i);
    expect(cloudCard).toHaveTextContent(/openai_api_key: missing/i);
    expect(cloudCard).toHaveTextContent(/missing gemini_api_key/i);
    expect(cloudCard).toHaveTextContent(/missing openai_api_key/i);
    expect(screen.getAllByLabelText(/API key/i)).toHaveLength(2);
    expect(
      screen.getAllByText(
        /add a key here to make the selected provider path runnable/i,
      ),
    ).toHaveLength(2);
    expect(screen.getByRole("button", { name: /save setup/i })).toBeDisabled();
  });

  it("renders source blockers from the shared setup mode cards", async () => {
    const onDismiss = vi.fn();
    useAudioGraphStore.setState({
      audioSources: [selectedSystemSource()],
      selectedSourceIds: [],
    });
    mockProviderState({
      presence: credentialPresence("gemini_api_key", "openai_api_key"),
      readiness: [
        readyProvider("asr.api", ["gemini_api_key"]),
        readyProvider("llm.api", ["openai_api_key"]),
      ],
    });

    render(<ExpressSetup onDismiss={onDismiss} onOpenAdvanced={() => {}} />);

    await waitFor(() =>
      expect(screen.queryByLabelText(/API key/i)).not.toBeInTheDocument(),
    );
    const cloudCard = screen.getByTestId("express-mode-card-cloud_fast");
    expect(cloudCard).toHaveTextContent(/Cloud fast \(selected\)/i);
    expect(cloudCard).toHaveTextContent(/Blocked/i);
    expect(cloudCard).toHaveTextContent(/needs an audio source selection/i);
    expect(
      within(cloudCard).getByText(/source picker before starting capture/i),
    ).toBeInTheDocument();
    fireEvent.click(
      within(cloudCard).getByRole("button", { name: /review sources/i }),
    );
    expect(onDismiss).toHaveBeenCalled();
    expect(useAudioGraphStore.getState().sourceRecoveryIntent).toMatchObject({
      origin: "provider_setup",
      issues: [
        expect.objectContaining({
          kind: "unselected",
          message: expect.stringMatching(/needs an audio source selection/i),
        }),
      ],
    });
  });

  it("saves credentials + settings and dismisses when Save setup is clicked", async () => {
    const onDismiss = vi.fn();
    render(<ExpressSetup onDismiss={onDismiss} onOpenAdvanced={() => {}} />);

    // Switch to Deepgram so we can assert the deepgram_api_key slot.
    const asrSelect = screen.getByLabelText(
      /ASR \(speech-to-text\) provider/i,
    ) as HTMLSelectElement;
    fireEvent.change(asrSelect, { target: { value: "deepgram" } });

    const asrKey = screen.getByLabelText(/ASR API key/i);
    fireEvent.change(asrKey, { target: { value: "dg-key" } });

    const llmKey = screen.getByLabelText(/LLM API key/i);
    fireEvent.change(llmKey, { target: { value: "sk-openai" } });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save setup/i }));
    });

    // We expect save_credential_cmd for Deepgram + OpenAI, and a
    // save_settings_cmd containing the Deepgram ASR provider.
    const credKeys = savedCredentialKeys();
    expect(credKeys).toContain("deepgram_api_key");
    expect(credKeys).toContain("openai_api_key");

    const settingsArg = savedSettingsArg();
    expect(settingsArg.asr_provider.type).toBe("deepgram");
    expect(settingsArg.audio_settings).toEqual({
      sample_rate: 48000,
      channels: 2,
    });

    expect(onDismiss).toHaveBeenCalled();
  });

  it("saves Gemini API ASR as an OpenAI-compatible durable route without starting capture", async () => {
    const onDismiss = vi.fn();
    render(<ExpressSetup onDismiss={onDismiss} onOpenAdvanced={() => {}} />);

    fireEvent.change(screen.getByLabelText(/ASR API key/i), {
      target: { value: "gemini-api-key" },
    });
    fireEvent.change(screen.getByLabelText(/LLM API key/i), {
      target: { value: "sk-openai" },
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save setup/i }));
    });

    expect(savedCredentialKeys()).toEqual(["gemini_api_key", "openai_api_key"]);
    expect(savedSettingsArg().asr_provider).toEqual({
      type: "api",
      endpoint: GEMINI_OPENAI_ENDPOINT,
      api_key: "",
      model: "gemini-2.5-flash",
    });
    expect(
      mockedInvoke.mock.calls.some(([cmd]) => cmd === "load_credential_cmd"),
    ).toBe(false);
    expect(
      mockedInvoke.mock.calls.some(([cmd]) =>
        [
          "start_capture",
          "start_transcribe",
          "start_gemini",
          "start_converse",
        ].includes(cmd),
      ),
    ).toBe(false);
    expect(JSON.stringify(savedSettingsArg())).not.toContain("gemini-api-key");
    expect(JSON.stringify(savedSettingsArg())).not.toContain("sk-openai");
    expect(onDismiss).toHaveBeenCalled();
  });

  it("reuses the Gemini ASR key when native Gemini Live is selected with Gemini ASR", async () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    fireEvent.click(
      screen.getByLabelText(/configure native gemini live realtime mode/i),
    );
    expect(
      screen.queryByLabelText(/Gemini Live API key/i),
    ).not.toBeInTheDocument();

    fireEvent.change(screen.getByLabelText(/ASR API key/i), {
      target: { value: "gemini-api-key" },
    });
    fireEvent.change(screen.getByLabelText(/LLM API key/i), {
      target: { value: "sk-openai" },
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save setup/i }));
    });

    expect(savedCredentialKeys()).toEqual(["gemini_api_key", "openai_api_key"]);
    expect(savedSettingsArg().gemini.auth).toEqual({
      type: "api_key",
      api_key: "",
    });
    expect(JSON.stringify(savedSettingsArg())).not.toContain("gemini-api-key");
  });

  it("keeps native Gemini Live settings separate unless the Live checkbox is selected", async () => {
    useAudioGraphStore.setState({ settings: makeExistingSettings() });
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    fireEvent.change(screen.getByLabelText(/ASR API key/i), {
      target: { value: "gemini-api-key" },
    });
    fireEvent.change(screen.getByLabelText(/LLM API key/i), {
      target: { value: "sk-openai" },
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save setup/i }));
    });

    expect(savedSettingsArg().gemini).toEqual(makeExistingSettings().gemini);
  });

  it("saves the optional Gemini Live key only from the native realtime opt-in", async () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    const asrSelect = screen.getByLabelText(
      /ASR \(speech-to-text\) provider/i,
    ) as HTMLSelectElement;
    fireEvent.change(asrSelect, { target: { value: "deepgram" } });
    fireEvent.change(screen.getByLabelText(/ASR API key/i), {
      target: { value: "dg-key" },
    });
    fireEvent.change(screen.getByLabelText(/LLM API key/i), {
      target: { value: "sk-openai" },
    });
    fireEvent.click(
      screen.getByLabelText(/configure native gemini live realtime mode/i),
    );
    fireEvent.change(screen.getByLabelText(/Gemini Live API key/i), {
      target: { value: "gemini-live-key" },
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save setup/i }));
    });

    expect(savedCredentialKeys()).toEqual([
      "deepgram_api_key",
      "openai_api_key",
      "gemini_api_key",
    ]);
    expect(savedSettingsArg().asr_provider.type).toBe("deepgram");
    expect(savedSettingsArg().gemini.auth).toEqual({
      type: "api_key",
      api_key: "",
    });
    expect(JSON.stringify(savedSettingsArg())).not.toContain("gemini-live-key");
  });

  it("dismisses without saving on Skip setup and on Escape", () => {
    const onDismiss = vi.fn();
    const { unmount } = render(
      <ExpressSetup onDismiss={onDismiss} onOpenAdvanced={() => {}} />,
    );
    // Two elements have "skip setup" accessible names (header ✕ and
    // footer button). The footer button is the user-visible text-only one.
    const skipButtons = screen.getAllByRole("button", {
      name: /skip setup/i,
    });
    const footerSkip = skipButtons.find(
      (b) => b.textContent?.trim() === "Skip setup",
    );
    expect(footerSkip).toBeDefined();
    fireEvent.click(footerSkip as HTMLElement);
    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(
      mockedInvoke.mock.calls.filter(([cmd]) => cmd === "save_credential_cmd"),
    ).toHaveLength(0);

    unmount();
    const onDismiss2 = vi.fn();
    render(<ExpressSetup onDismiss={onDismiss2} onOpenAdvanced={() => {}} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onDismiss2).toHaveBeenCalledTimes(1);
  });

  it("starts the sample-session preview without saving settings or credentials", () => {
    const onPreviewSampleSession = vi.fn();
    render(
      <ExpressSetup
        onDismiss={() => {}}
        onOpenAdvanced={() => {}}
        onPreviewSampleSession={onPreviewSampleSession}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: /preview sample session/i }),
    );

    expect(onPreviewSampleSession).toHaveBeenCalledTimes(1);
    expect(
      mockedInvoke.mock.calls.some(([cmd]) =>
        ["save_credential_cmd", "save_settings_cmd"].includes(cmd),
      ),
    ).toBe(false);
  });

  it("localizes the Setup modes section under pt (seed 88ad — no hardcoded English)", async () => {
    await i18n.changeLanguage("pt");
    try {
      render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

      // The section aria-label + its metadata dt labels + the no-key blocker
      // summary must all render the pt strings, not the old hardcoded English.
      const section = screen.getByRole("region", {
        name: /modos de configuração/i,
      });
      expect(section).toBeInTheDocument();
      expect(
        within(section).getAllByText(/limite de dados/i).length,
      ).toBeGreaterThan(0);
      expect(
        within(section).getAllByText(/caminho do produto/i).length,
      ).toBeGreaterThan(0);
      // Selected card localizes "(selected)" → "(selecionado)".
      const cloudCard = screen.getByTestId("express-mode-card-cloud_fast");
      expect(cloudCard).toHaveTextContent(/\(selecionado\)/i);
      expect(cloudCard).toHaveTextContent(/credenciais ausentes/i);
      // The old hardcoded English must be gone from the section.
      expect(within(section).queryByText(/^Data boundary$/)).toBeNull();
      expect(within(section).queryByText(/^Product path$/)).toBeNull();
    } finally {
      await i18n.changeLanguage("en");
    }
  });
});
