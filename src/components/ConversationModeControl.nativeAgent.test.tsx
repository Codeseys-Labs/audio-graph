import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import type { AppSettings, GeminiSettings } from "../types";
import ConversationModeControl from "./ConversationModeControl";

// Settings T1 (seed audio-graph-2b9a): `ConversationModeControl.tsx:164`'s
// "Configure Gemini API key" badge action still calls `openSettings()` bare
// under the widened `openSettings(route?)` signature. That branch only
// renders when `nativeAgentSelectable` is true, i.e. at least one
// realtime-agent provider is `ui_selectable` in the generated registry — and
// today (MVP scoping, audio-graph-ad56) both `realtime_agent.gemini_live`
// and `realtime_agent.openai_realtime` are `ui_selectable: false`, so the
// branch is unreachable under the real registry (see
// `ConversationModeControl.test.tsx`'s "deferred" tests, which assert the
// button's absence for exactly this reason). Isolating the registry
// override to its own file keeps `ConversationModeControl.test.tsx`'s
// real-registry coverage (Native disabled, Configure action absent) intact.
vi.mock("../generated/providerRegistry", async () => {
  const actual = await vi.importActual<
    typeof import("../generated/providerRegistry")
  >("../generated/providerRegistry");
  return {
    GENERATED_PROVIDER_REGISTRY: actual.GENERATED_PROVIDER_REGISTRY.map(
      (provider) =>
        provider.id === "realtime_agent.gemini_live"
          ? { ...provider, ui_selectable: true }
          : provider,
    ),
  };
});

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
      auth: { type: "none" } as unknown as GeminiSettings["auth"],
      model: "gemini-3.1-flash-live-preview",
    },
    log_level: "info",
    ...overrides,
  };
}

function resetStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    conversationMode: "converse",
    setConversationMode: vi.fn(),
    converseEngine: "native",
    setConverseEngine: vi.fn(),
    settings: makeSettings(),
    openSettings: vi.fn(),
    ...overrides,
  });
}

describe("ConversationModeControl (native realtime-agent selectable)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    resetStore();
  });

  it("clicking Configure navigates bare (openSettings called with no route)", () => {
    const openSettings = vi.fn();
    resetStore({ openSettings });
    render(<ConversationModeControl />);
    fireEvent.click(
      screen.getByRole("button", { name: "Configure Gemini API key" }),
    );
    expect(openSettings).toHaveBeenCalledWith();
  });
});
