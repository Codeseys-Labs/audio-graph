import { fireEvent, render, screen } from "@testing-library/react";
import i18n from "i18next";
import { describe, expect, it, vi } from "vitest";
import AudioSettings from "./AudioSettings";
import type { ChannelCount, SampleRate } from "./settingsTypes";

// The component is a pure, prop-driven sub-form: it reads the two audio fields
// off `state` and dispatches `setField(...)` actions on change. `t` is the real
// i18next instance (initialized in test/setup.ts) so labels render English copy.
const t = i18n.t.bind(i18n);

function renderAudioSettings(
  overrides: {
    audioSampleRate?: SampleRate;
    audioChannels?: ChannelCount;
  } = {},
) {
  const dispatch = vi.fn();
  render(
    <AudioSettings
      state={{
        audioSampleRate: overrides.audioSampleRate ?? 48000,
        audioChannels: overrides.audioChannels ?? 2,
      }}
      dispatch={dispatch}
      t={t}
    />,
  );
  return { dispatch };
}

describe("AudioSettings", () => {
  it("renders the Audio section heading", () => {
    renderAudioSettings();
    expect(screen.getByRole("heading", { name: /audio/i })).toBeInTheDocument();
  });

  it("renders the sample-rate and channels selects with accessible labels", () => {
    renderAudioSettings();
    expect(screen.getByLabelText(/capture sample rate/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/capture channels/i)).toBeInTheDocument();
  });

  it("reflects the sample rate from props as the selected value", () => {
    renderAudioSettings({ audioSampleRate: 96000 });
    expect(screen.getByLabelText(/capture sample rate/i)).toHaveValue("96000");
  });

  it("reflects the channel count from props as the selected value", () => {
    renderAudioSettings({ audioChannels: 2 });
    expect(screen.getByLabelText(/capture channels/i)).toHaveValue("2");
  });

  it("dispatches a numeric audioSampleRate SET_FIELD on sample-rate change", () => {
    const { dispatch } = renderAudioSettings();
    fireEvent.change(screen.getByLabelText(/capture sample rate/i), {
      target: { value: "44100" },
    });
    expect(dispatch).toHaveBeenCalledWith({
      type: "SET_FIELD",
      field: "audioSampleRate",
      value: 44100,
    });
  });

  it("dispatches a numeric audioChannels SET_FIELD on channels change", () => {
    const { dispatch } = renderAudioSettings({ audioChannels: 1 });
    fireEvent.change(screen.getByLabelText(/capture channels/i), {
      target: { value: "2" },
    });
    expect(dispatch).toHaveBeenCalledWith({
      type: "SET_FIELD",
      field: "audioChannels",
      value: 2,
    });
  });

  it("offers all six supported capture sample rates", () => {
    renderAudioSettings();
    const select = screen.getByLabelText(/capture sample rate/i);
    const values = Array.from(
      select.querySelectorAll("option"),
      (o) => (o as HTMLOptionElement).value,
    );
    expect(values).toEqual([
      "22050",
      "32000",
      "44100",
      "48000",
      "88200",
      "96000",
    ]);
  });

  it("renders the downmix hint", () => {
    renderAudioSettings();
    expect(screen.getByText(/downmixes to 16 kHz mono/i)).toBeInTheDocument();
  });
});
