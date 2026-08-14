import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import type { AppSettings, GeminiSettings } from "../types";
import ConversationModeControl from "./ConversationModeControl";

// Minimal AppSettings fixture; only `gemini.auth` and `llm_provider` gate the
// availability badges this control renders.
function makeSettings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    asr_provider: { type: "local_whisper" },
    tts_provider: { type: "none" },
    speak_aloud: false,
    whisper_model: "ggml-small.en.bin",
    llm_provider: { type: "local_llama" },
    llm_api_config: null,
    audio_settings: { sample_rate: 48000, channels: 1 },
    gemini: {
      auth: { type: "api_key", api_key: "key" },
      model: "gemini-3.1-flash-live-preview",
    },
    log_level: "info",
    ...overrides,
  };
}

// Build a Gemini settings object whose auth type is neither api_key nor
// vertex_ai, exercising the "no key configured" availability branch. The
// auth union is closed in the type system, so we cast the runtime shape.
function noGeminiKey(): GeminiSettings {
  return {
    auth: { type: "none" } as unknown as GeminiSettings["auth"],
    model: "gemini-3.1-flash-live-preview",
  };
}

function resetStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    conversationMode: "notes",
    setConversationMode: vi.fn(),
    converseEngine: "pipelined",
    setConverseEngine: vi.fn(),
    settings: makeSettings(),
    openSettings: vi.fn(),
    ...overrides,
  });
}

describe("ConversationModeControl", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    resetStore();
  });

  it("renders Notes + Converse as a labeled pressed-button group", () => {
    render(<ConversationModeControl />);
    expect(
      screen.getByRole("group", { name: /conversation mode/i }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /notes/i })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /converse/i }),
    ).toBeInTheDocument();
  });

  it("marks Notes selected and Converse unselected when mode is 'notes'", () => {
    resetStore({ conversationMode: "notes" });
    render(<ConversationModeControl />);
    expect(screen.getByRole("button", { name: /notes/i })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: /converse/i })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("does not render the engine picker while in Notes mode", () => {
    resetStore({ conversationMode: "notes" });
    render(<ConversationModeControl />);
    expect(
      screen.queryByRole("button", { name: /pipelined/i }),
    ).not.toBeInTheDocument();
  });

  it("clicking the Converse tab calls setConversationMode('converse')", () => {
    const setConversationMode = vi.fn();
    resetStore({ setConversationMode });
    render(<ConversationModeControl />);
    fireEvent.click(screen.getByRole("button", { name: /converse/i }));
    expect(setConversationMode).toHaveBeenCalledWith("converse");
  });

  it("clicking the Notes tab calls setConversationMode('notes')", () => {
    const setConversationMode = vi.fn();
    resetStore({ conversationMode: "converse", setConversationMode });
    render(<ConversationModeControl />);
    fireEvent.click(screen.getByRole("button", { name: /notes/i }));
    expect(setConversationMode).toHaveBeenCalledWith("notes");
  });

  it("reveals the Pipelined + Native engine buttons in Converse mode", () => {
    resetStore({ conversationMode: "converse" });
    render(<ConversationModeControl />);
    expect(
      screen.getByRole("button", { name: /pipelined/i }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /native/i })).toBeInTheDocument();
  });

  it("reflects the selected engine via aria-pressed", () => {
    resetStore({ conversationMode: "converse", converseEngine: "pipelined" });
    render(<ConversationModeControl />);
    expect(screen.getByRole("button", { name: /pipelined/i })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: /native/i })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("keeps Native visible but disabled while its providers are outside the MVP", () => {
    const setConverseEngine = vi.fn();
    resetStore({ conversationMode: "converse", setConverseEngine });
    render(<ConversationModeControl />);
    const native = screen.getByRole("button", { name: /native/i });
    expect(native).toBeDisabled();
    expect(native).toHaveTextContent(/not in mvp/i);
    fireEvent.click(native);
    expect(setConverseEngine).not.toHaveBeenCalled();
  });

  it("clicking the Pipelined engine button calls setConverseEngine('pipelined')", () => {
    const setConverseEngine = vi.fn();
    resetStore({
      conversationMode: "converse",
      converseEngine: "native",
      setConverseEngine,
    });
    render(<ConversationModeControl />);
    fireEvent.click(screen.getByRole("button", { name: /pipelined/i }));
    expect(setConverseEngine).toHaveBeenCalledWith("pipelined");
  });

  it("shows a 'Needs setup' badge on Pipelined when no LLM provider is configured", () => {
    // `hasLlm` is computed as `Boolean(settings?.llm_provider)`, so the
    // missing-LLM branch is reached when llm_provider is absent. The field
    // is non-optional in the type, so cast through the maker's overrides.
    resetStore({
      conversationMode: "converse",
      settings: makeSettings({
        llm_provider: undefined as unknown as AppSettings["llm_provider"],
      }),
    });
    render(<ConversationModeControl />);
    expect(screen.getByText(/needs setup/i)).toBeInTheDocument();
  });

  it("omits the 'Needs setup' badge on Pipelined when an LLM provider exists", () => {
    resetStore({
      conversationMode: "converse",
      settings: makeSettings({ llm_provider: { type: "local_llama" } }),
    });
    render(<ConversationModeControl />);
    expect(screen.queryByText(/needs setup/i)).not.toBeInTheDocument();
  });

  it("does not offer credential setup for a deferred Native provider", () => {
    resetStore({
      conversationMode: "converse",
      settings: makeSettings({ gemini: noGeminiKey() }),
    });
    render(<ConversationModeControl />);
    expect(
      screen.queryByRole("button", { name: "Configure Gemini API key" }),
    ).not.toBeInTheDocument();
  });

  it("does not render deferred realtime-agent provider controls", () => {
    const openSettings = vi.fn();
    const setConverseEngine = vi.fn();
    resetStore({
      conversationMode: "converse",
      converseEngine: "native",
      setConverseEngine,
      openSettings,
      settings: makeSettings({ gemini: noGeminiKey() }),
    });
    render(<ConversationModeControl />);
    expect(
      screen.queryByRole("group", { name: /realtime agent provider/i }),
    ).not.toBeInTheDocument();
    expect(openSettings).not.toHaveBeenCalled();
    expect(setConverseEngine).not.toHaveBeenCalled();
  });

  it("hides the Configure action when a Gemini api_key is configured", () => {
    resetStore({
      conversationMode: "converse",
      settings: makeSettings({
        gemini: {
          auth: { type: "api_key", api_key: "abc" },
          model: "gemini-3.1-flash-live-preview",
        },
      }),
    });
    render(<ConversationModeControl />);
    expect(
      screen.queryByRole("button", { name: /configure/i }),
    ).not.toBeInTheDocument();
  });

  it("treats vertex_ai auth as having a Gemini key (no Configure action)", () => {
    resetStore({
      conversationMode: "converse",
      settings: makeSettings({
        gemini: {
          auth: {
            type: "vertex_ai",
            project_id: "p",
            location: "us-central1",
          },
          model: "gemini-3.1-flash-live-preview",
        },
      }),
    });
    render(<ConversationModeControl />);
    expect(
      screen.queryByRole("button", { name: /configure/i }),
    ).not.toBeInTheDocument();
  });
});
