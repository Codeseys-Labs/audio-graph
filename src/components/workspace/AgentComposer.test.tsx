import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "../../store";
import type { AppSettings } from "../../types";
import { AgentComposer } from "./AgentComposer";

/**
 * `AutoAnswerCountChip` (audio-graph-83cc T5, deliverable e; fix round —
 * adversarial + scope-honesty minor: "no rendering test") — pins the three
 * user-visible behaviors the implementer's own doc comment claims but that
 * nothing previously exercised:
 *   1. hidden entirely while `agent_auto_answer.enabled` is not `=== true`
 *      (absent settings, or an explicit `false`)
 *   2. renders "{{count}}/{{cap}}" from `autoAnswerDispatchCount` +
 *      `agent_auto_answer.max_per_session` when enabled
 *   3. carries the sr-only aria description alongside the aria-hidden label
 */

function settingsWithAutoAnswer(
  overrides: Partial<AppSettings["agent_auto_answer"]> = {},
): AppSettings {
  return {
    asr_provider: {
      type: "deepgram",
      model: "nova-3",
      enable_diarization: true,
    },
    whisper_model: "ggml-small.en.bin",
    llm_provider: {
      type: "openrouter",
      model: "openai/gpt-4.1-mini",
      base_url: "https://openrouter.ai/api/v1",
      include_usage_in_stream: true,
    },
    llm_api_config: null,
    audio_settings: { sample_rate: 48_000, channels: 1 },
    gemini: {
      auth: { type: "api_key" },
      model: "gemini-2.0-flash-live-001",
    },
    tts_provider: { type: "none" },
    speak_aloud: false,
    log_level: "info",
    agent_auto_answer: {
      enabled: true,
      max_per_session: 12,
      min_interval_secs: 45,
      ...overrides,
    },
  };
}

describe("AutoAnswerCountChip (via AgentComposer)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      settings: null,
      autoAnswerDispatchCount: 0,
      composerError: null,
    });
  });

  it("is absent when settings have not loaded yet", () => {
    render(<AgentComposer />);
    expect(screen.queryByText(/\d+\/\d+/)).toBeNull();
  });

  it("is absent when agent_auto_answer.enabled is false (belt check mirrors the FE trigger's own belt)", () => {
    useAudioGraphStore.setState({
      settings: settingsWithAutoAnswer({ enabled: false }),
    });
    render(<AgentComposer />);
    expect(screen.queryByText(/\d+\/\d+/)).toBeNull();
  });

  it("renders count/cap from the store when enabled", () => {
    useAudioGraphStore.setState({
      settings: settingsWithAutoAnswer({ max_per_session: 12 }),
      autoAnswerDispatchCount: 3,
    });
    render(<AgentComposer />);
    expect(screen.getByText("3/12")).toBeInTheDocument();
  });

  it("falls back to a 12 cap before settings finish loading the real value, and updates live as the dispatch count changes", () => {
    useAudioGraphStore.setState({
      settings: settingsWithAutoAnswer(),
      autoAnswerDispatchCount: 0,
    });
    const { rerender } = render(<AgentComposer />);
    expect(screen.getByText("0/12")).toBeInTheDocument();

    useAudioGraphStore.setState({ autoAnswerDispatchCount: 1 });
    rerender(<AgentComposer />);
    expect(screen.getByText("1/12")).toBeInTheDocument();
  });

  it("carries an sr-only description alongside the aria-hidden visible label", () => {
    useAudioGraphStore.setState({
      settings: settingsWithAutoAnswer(),
      autoAnswerDispatchCount: 5,
    });
    render(<AgentComposer />);
    const visible = screen.getByText("5/12");
    expect(visible).toHaveAttribute("aria-hidden", "true");
    // The sr-only sibling carries the full sentence-form description, not
    // the terse "5/12" — distinct text nodes prove both spans rendered.
    expect(screen.queryByText("5/12", { selector: ".sr-only" })).toBeNull();
    expect(document.querySelector(".sr-only")?.textContent).not.toHaveLength(0);
  });
});
