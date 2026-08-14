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
} from "../types";
import ExpressSetup from "./ExpressSetup";

const mockedInvoke = vi.mocked(invoke);
const originalSetConversationMode =
  useAudioGraphStore.getState().setConversationMode;
const originalSetConverseEngine =
  useAudioGraphStore.getState().setConverseEngine;

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
): ProviderReadiness => ({
  provider_id: providerId,
  status: "ready",
  message: `${providerId} ready`,
  stale: false,
  credential_epoch: 1,
  credentials: credentials.map((key) => ({ key, present: true })),
  model_catalog: [],
  runtime: null,
});

function failPlaintextCredentialLoadback(args?: unknown): never {
  void args;
  throw new Error(
    "load_credential_cmd should not be invoked by frontend tests; use load_credential_presence_cmd instead.",
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
      setConversationMode: originalSetConversationMode,
      setConverseEngine: originalSetConverseEngine,
    });
    // Default: any save_* command succeeds; saved-key state comes from
    // credential presence, never plaintext credential loadback.
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
    // MVP scoping (audio-graph-ad56): the ASR quickstart defaults to
    // Deepgram — the only ui_selectable ASR provider.
    expect(
      screen.getByLabelText(/ASR \(speech-to-text\) provider/i),
    ).toHaveValue("deepgram");
    // Both cloud providers need a key → there are two API key inputs.
    expect(screen.getAllByLabelText(/API key/i)).toHaveLength(2);
    expect(
      screen.getAllByText(
        /add a key here to make the selected provider path runnable/i,
      ),
    ).toHaveLength(2);
  });

  it("loads credential presence without provider readiness on mount or focus", async () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    await waitFor(() =>
      expect(
        mockedInvoke.mock.calls.filter(
          ([cmd]) => cmd === "load_credential_presence_cmd",
        ),
      ).toHaveLength(1),
    );
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd]) => cmd === "get_provider_readiness_cmd",
      ),
    ).toBe(false);

    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });

    await waitFor(() =>
      expect(
        mockedInvoke.mock.calls.filter(
          ([cmd]) => cmd === "load_credential_presence_cmd",
        ),
      ).toHaveLength(2),
    );
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd]) => cmd === "get_provider_readiness_cmd",
      ),
    ).toBe(false);
  });

  it("offers only ui_selectable providers as Express choices (MVP scoping)", () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);
    const asrSelect = screen.getByLabelText(
      /ASR \(speech-to-text\) provider/i,
    ) as HTMLSelectElement;
    const asrValues = Array.from(asrSelect.options).map((o) => o.value);
    // Deferred providers (gemini→asr.api, assemblyai, local_whisper) must
    // not be offered on the quickstart path either — same axis as Settings.
    expect(asrValues).toEqual(["deepgram"]);

    const llmSelect = screen.getByLabelText(
      /LLM \(chat\) provider/i,
    ) as HTMLSelectElement;
    const llmValues = Array.from(llmSelect.options).map((o) => o.value);
    // Anthropic uses a transport shape that is not implemented by the MVP;
    // it must not masquerade as the generic llm.api route.
    expect(llmValues).toEqual(["openai", "local_llama", "openrouter"]);
  });

  it("disables Save setup until required cloud keys are filled", () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);
    const save = screen.getByRole("button", { name: /save setup/i });
    expect(save).toBeDisabled();

    // Fill ASR key (Deepgram by default under MVP scoping).
    const asrKey = screen.getByLabelText(/ASR API key/i);
    fireEvent.change(asrKey, { target: { value: "test-key-not-real" } });
    expect(save).toBeDisabled(); // LLM still missing.

    const llmKey = screen.getByLabelText(/LLM API key/i);
    fireEvent.change(llmKey, { target: { value: "test-key-not-real-2" } });
    expect(save).toBeEnabled();
  });

  it("uses saved OpenRouter presence without prompting for plaintext", async () => {
    // MVP scoping (audio-graph-ad56): local_whisper is deferred, so the
    // hybrid (local-ASR) card is no longer reachable from Express choices.
    // The subject under test — saved OpenRouter presence means no plaintext
    // key prompt and no credential readback — now runs on the cloud pair
    // (Deepgram default + OpenRouter), with saved presence on both sides.
    mockProviderState({
      presence: credentialPresence("openrouter_api_key", "deepgram_api_key"),
      readiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
    });
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    fireEvent.change(
      screen.getByLabelText(/LLM \(chat\) provider/i) as HTMLSelectElement,
      { target: { value: "openrouter" } },
    );

    await waitFor(() =>
      expect(screen.queryByLabelText(/LLM API key/i)).not.toBeInTheDocument(),
    );
    expect(screen.queryByLabelText(/ASR API key/i)).not.toBeInTheDocument();
    expect(
      screen.getAllByText(/backend will use it without re-entry/i).length,
    ).toBeGreaterThanOrEqual(1);

    const cloudCard = screen.getByTestId("express-mode-card-cloud_fast");
    expect(cloudCard).toHaveTextContent(/Cloud fast \(selected\)/i);
    expect(cloudCard).toHaveTextContent(/openrouter_api_key: present/i);
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

  it("keeps persisted readiness from labeling an unsaved draft Ready or Error", async () => {
    mockProviderState({
      presence: credentialPresence(
        "gemini_api_key",
        "openai_api_key",
        "deepgram_api_key",
      ),
      readiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        {
          ...readyProvider("llm.api", ["openai_api_key"]),
          status: "error",
          message: "persisted route failed",
        },
      ],
    });
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    await waitFor(() =>
      expect(screen.queryByLabelText(/ASR API key/i)).not.toBeInTheDocument(),
    );
    expect(screen.queryByLabelText(/LLM API key/i)).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(/configure native gemini live realtime mode/i),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(/Gemini Live API key/i),
    ).not.toBeInTheDocument();

    const cloudCard = screen.getByTestId("express-mode-card-cloud_fast");
    const draftProviderRows = within(cloudCard)
      .getAllByRole("listitem")
      .filter((row) =>
        /Deepgram streaming|OpenAI-compatible LLM/i.test(row.textContent ?? ""),
      );
    expect(draftProviderRows).toHaveLength(2);
    for (const row of draftProviderRows) {
      expect(row).toHaveTextContent(/Unchecked/i);
      expect(row).not.toHaveTextContent(/Ready/i);
      expect(row).not.toHaveTextContent(/Error/i);
    }
    expect(cloudCard).not.toHaveTextContent(/persisted route failed/i);
    expect(
      mockedInvoke.mock.calls.some(
        ([cmd]) => cmd === "get_provider_readiness_cmd",
      ),
    ).toBe(false);
  });

  it("re-probes credential presence on window focus so the runnable-pair gate can't go stale", async () => {
    // cred-review M2.2 / a8db: the Save gate derives from credentialPresence,
    // fetched once at mount. If a key is cleared in Settings (via the Advanced
    // round-trip) while ExpressSetup stays open, a mount-only probe would keep
    // the gate green against a key that no longer exists. Focus re-probe fixes
    // it: refetch on window focus and re-derive the gate.
    let presence: CredentialPresence[] = credentialPresence(
      "deepgram_api_key",
      "openai_api_key",
    );
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") return presence;
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
      return undefined;
    });

    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    fireEvent.change(
      screen.getByLabelText(
        /ASR \(speech-to-text\) provider/i,
      ) as HTMLSelectElement,
      { target: { value: "deepgram" } },
    );

    // Both stages covered by saved keys → Save is enabled.
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /save setup/i })).toBeEnabled(),
    );

    // Simulate the user clearing the LLM key in Settings and returning: the
    // next presence probe no longer reports openai_api_key.
    presence = credentialPresence("deepgram_api_key");

    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });

    // The gate refreshed off the new presence and re-blocked Save.
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /save setup/i }),
      ).toBeDisabled(),
    );
  });

  it("ignores a late stale credential-presence response after a newer refresh resolved", async () => {
    // PR #84 review (Codex P2): the mount probe and the focus re-probe race.
    // Each used to check only its own unmount flag, so a SLOW old response
    // resolving LAST would overwrite fresher credentialPresence — re-enabling
    // Save against a key the newer probe already reported gone. Request-id
    // ordering drops the out-of-order write. Here: refresh #1 (mount) hangs on
    // a deferred promise carrying STALE "both keys present" data; refresh #2
    // (focus) resolves immediately with "openai_api_key gone"; then #1 lands.
    let releaseStalePresence: (value: CredentialPresence[]) => void = () => {};
    const stalePresence = new Promise<CredentialPresence[]>((resolve) => {
      releaseStalePresence = resolve;
    });

    let presenceCall = 0;
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        presenceCall += 1;
        // Call #1 (mount): hangs until we release it with stale data.
        if (presenceCall === 1) return stalePresence;
        // Call #2+ (focus): resolves immediately with the key gone.
        return credentialPresence("deepgram_api_key");
      }
      if (cmd === "load_credential_cmd") failPlaintextCredentialLoadback();
      return undefined;
    });

    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    fireEvent.change(
      screen.getByLabelText(
        /ASR \(speech-to-text\) provider/i,
      ) as HTMLSelectElement,
      { target: { value: "deepgram" } },
    );

    // Newer refresh (focus) resolves first: the LLM key is gone → Save blocked.
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /save setup/i }),
      ).toBeDisabled(),
    );

    // Now the OLD mount-time response finally lands, claiming both keys are
    // still present. It must be discarded — Save stays blocked. (Direct
    // assertion, not waitFor: act() has already flushed the stale write, and
    // waitFor would pass on its first pre-write check even without the guard.)
    await act(async () => {
      releaseStalePresence(
        credentialPresence("deepgram_api_key", "openai_api_key"),
      );
    });
    expect(screen.getByRole("button", { name: /save setup/i })).toBeDisabled();
  });

  it("keeps native realtime unselected when only the legacy flag is stale", async () => {
    useAudioGraphStore.setState({
      nativeS2sEnabled: true,
      conversationMode: "notes",
      converseEngine: "native",
    });
    mockProviderState({
      presence: credentialPresence(
        "gemini_api_key",
        "openai_api_key",
        "deepgram_api_key",
      ),
    });

    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    await waitFor(() =>
      expect(screen.queryByLabelText(/ASR API key/i)).not.toBeInTheDocument(),
    );
    const cloudCard = screen.getByTestId("express-mode-card-cloud_fast");
    expect(cloudCard).toHaveTextContent(/Cloud fast \(selected\)/i);
    const nativeCard = screen.getByTestId("express-mode-card-native_realtime");
    expect(nativeCard).not.toHaveTextContent(/Native realtime \(selected\)/i);
    expect(
      screen.queryByLabelText(/configure native gemini live realtime mode/i),
    ).not.toBeInTheDocument();
  });

  it("shows no-key blockers from provider setup mode cards", () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    const cloudCard = screen.getByTestId("express-mode-card-cloud_fast");
    expect(cloudCard).toHaveTextContent(/Cloud fast \(selected\)/i);
    expect(cloudCard).toHaveTextContent(/Missing credentials/i);
    // MVP scoping (ad56): the ASR default is deepgram, so the missing pair
    // is deepgram + openai.
    expect(cloudCard).toHaveTextContent(/deepgram_api_key: missing/i);
    expect(cloudCard).toHaveTextContent(/openai_api_key: missing/i);
    expect(cloudCard).toHaveTextContent(/missing deepgram_api_key/i);
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
      // MVP scoping (ad56): deepgram is the ASR default; keep openai for LLM.
      presence: credentialPresence("deepgram_api_key", "openai_api_key"),
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
    const setConversationMode = vi.fn(originalSetConversationMode);
    const setConverseEngine = vi.fn(originalSetConverseEngine);
    useAudioGraphStore.setState({ setConversationMode, setConverseEngine });
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
    expect(credKeys).toEqual(["deepgram_api_key", "openai_api_key"]);
    expect(credKeys).not.toContain("gemini_api_key");
    expect(credKeys).not.toContain("anthropic_api_key");

    const settingsArg = savedSettingsArg();
    expect(settingsArg.asr_provider.type).toBe("deepgram");
    expect(settingsArg.audio_settings).toEqual({
      sample_rate: 48000,
      channels: 2,
    });
    expect(useAudioGraphStore.getState().conversationMode).toBe("notes");
    expect(useAudioGraphStore.getState().converseEngine).toBe("pipelined");
    expect(setConversationMode).toHaveBeenCalledWith("notes");
    expect(setConverseEngine).toHaveBeenCalledWith("pipelined");

    expect(onDismiss).toHaveBeenCalled();
  });

  it("saves the default Deepgram route durably without starting capture", async () => {
    // Pre-MVP this test covered the Gemini→OpenAI-compatible durable route;
    // gemini is deferred (ad56), so the no-capture-on-save invariant is now
    // asserted on the Deepgram default path.
    const onDismiss = vi.fn();
    render(<ExpressSetup onDismiss={onDismiss} onOpenAdvanced={() => {}} />);

    fireEvent.change(screen.getByLabelText(/ASR API key/i), {
      target: { value: "test-key-not-real" },
    });
    fireEvent.change(screen.getByLabelText(/LLM API key/i), {
      target: { value: "test-key-not-real-2" },
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save setup/i }));
    });

    expect(savedCredentialKeys()).toEqual([
      "deepgram_api_key",
      "openai_api_key",
    ]);
    expect(savedSettingsArg().asr_provider.type).toBe("deepgram");
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
    expect(JSON.stringify(savedSettingsArg())).not.toContain(
      "test-key-not-real",
    );
    expect(onDismiss).toHaveBeenCalled();
  });

  it("does not offer deferred native Gemini Live controls", () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    expect(
      screen.queryByLabelText(/configure native gemini live realtime mode/i),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(/Gemini Live API key/i),
    ).not.toBeInTheDocument();
  });

  it("preserves existing native settings while saving the durable MVP route", async () => {
    useAudioGraphStore.setState({ settings: makeExistingSettings() });
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    fireEvent.change(screen.getByLabelText(/ASR API key/i), {
      target: { value: "test-key-not-real" },
    });
    fireEvent.change(screen.getByLabelText(/LLM API key/i), {
      target: { value: "test-key-not-real-2" },
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save setup/i }));
    });

    expect(savedSettingsArg().gemini).toEqual(makeExistingSettings().gemini);
    expect(savedCredentialKeys()).toEqual([
      "deepgram_api_key",
      "openai_api_key",
    ]);
    expect(useAudioGraphStore.getState().conversationMode).toBe("notes");
    expect(useAudioGraphStore.getState().converseEngine).toBe("pipelined");
  });

  it("never writes deferred Gemini Live or Anthropic credentials", async () => {
    render(<ExpressSetup onDismiss={() => {}} onOpenAdvanced={() => {}} />);

    fireEvent.change(screen.getByLabelText(/ASR API key/i), {
      target: { value: "dg-key" },
    });
    fireEvent.change(screen.getByLabelText(/LLM API key/i), {
      target: { value: "sk-openai" },
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /save setup/i }));
    });

    expect(savedCredentialKeys()).toEqual([
      "deepgram_api_key",
      "openai_api_key",
    ]);
    expect(savedCredentialKeys()).not.toContain("gemini_api_key");
    expect(savedCredentialKeys()).not.toContain("anthropic_api_key");
    expect(savedSettingsArg().asr_provider.type).toBe("deepgram");
    expect(JSON.stringify(savedSettingsArg())).not.toContain("anthropic");
    expect(useAudioGraphStore.getState().conversationMode).toBe("notes");
    expect(useAudioGraphStore.getState().converseEngine).toBe("pipelined");
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

  it("dismisses on Escape from a focused dialog descendant", async () => {
    const onDismiss = vi.fn();
    render(<ExpressSetup onDismiss={onDismiss} onOpenAdvanced={() => {}} />);

    const dialog = screen.getByRole("dialog", { name: /quick setup/i });
    const descendant = await within(dialog).findByLabelText(
      /ASR \(speech-to-text\) provider/i,
    );
    descendant.focus();
    expect(descendant).toHaveFocus();

    await act(async () => {
      fireEvent.keyDown(descendant, { key: "Escape" });
    });
    expect(onDismiss).toHaveBeenCalledTimes(1);
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
