import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, act, waitFor } from "@testing-library/react";
import DemoModeBanner from "./DemoModeBanner";
import { useAudioGraphStore } from "../store";
import type { AppSettings, ModelStatus } from "../types";
import "../i18n";

/**
 * Build an AppSettings fixture with `demo_mode` explicitly set. Only the
 * banner-relevant fields need to be realistic; the rest match the
 * store-level `baseSettings` shape used elsewhere in tests.
 */
function makeSettings(demo_mode: boolean | undefined): AppSettings {
    return {
        asr_provider: { type: "local_whisper" },
        whisper_model: "ggml-small.en.bin",
        llm_provider: { type: "local_llama" },
        llm_api_config: null,
        audio_settings: { sample_rate: 16000, channels: 1 },
        gemini: {
            auth: { type: "api_key", api_key: "" },
            model: "gemini-3.1-flash-live-preview",
        },
        log_level: "info",
        demo_mode,
    };
}

function resetStore(overrides: {
    settings?: AppSettings | null;
    modelStatus?: ModelStatus | null;
    openSettings?: () => void;
} = {}) {
    useAudioGraphStore.setState({
        settings: overrides.settings ?? null,
        modelStatus: overrides.modelStatus ?? null,
        openSettings: overrides.openSettings ?? vi.fn(),
        fetchSettings: vi.fn(async () => {}),
        fetchModelStatus: vi.fn(async () => {}),
    });
}

describe("DemoModeBanner", () => {
    beforeEach(() => {
        resetStore();
    });

    it("renders when demo_mode is true and no local models are ready", () => {
        resetStore({
            settings: makeSettings(true),
            modelStatus: {
                whisper: "NotDownloaded",
                llm: "NotDownloaded",
                sortformer: "NotDownloaded",
            },
        });

        render(<DemoModeBanner />);

        const banner = screen.getByTestId("demo-banner");
        expect(banner).toBeInTheDocument();
        expect(banner).toHaveAttribute("role", "status");
        expect(screen.getByText(/download whisper and llama/i)).toBeInTheDocument();
    });

    it("hides when both whisper and llama models report Ready", () => {
        resetStore({
            settings: makeSettings(true),
            modelStatus: {
                whisper: "Ready",
                llm: "Ready",
                // Sortformer is a diarization model and must NOT gate the
                // banner — the banner only cares about ASR + LLM readiness.
                sortformer: "NotDownloaded",
            },
        });

        render(<DemoModeBanner />);

        expect(screen.queryByTestId("demo-banner")).not.toBeInTheDocument();
    });

    it("stays visible when only one of the two required models is ready", () => {
        resetStore({
            settings: makeSettings(true),
            modelStatus: {
                whisper: "Ready",
                llm: "NotDownloaded",
                sortformer: "NotDownloaded",
            },
        });

        render(<DemoModeBanner />);

        // Half-downloaded is still unusable — the banner must keep nudging.
        expect(screen.getByTestId("demo-banner")).toBeInTheDocument();
    });

    it("renders nothing when demo_mode is false", () => {
        resetStore({
            settings: makeSettings(false),
            modelStatus: {
                whisper: "NotDownloaded",
                llm: "NotDownloaded",
                sortformer: "NotDownloaded",
            },
        });

        render(<DemoModeBanner />);

        expect(screen.queryByTestId("demo-banner")).not.toBeInTheDocument();
    });

    it("invokes openSettings when the action button is clicked", async () => {
        const openSettings = vi.fn();
        resetStore({
            settings: makeSettings(true),
            modelStatus: {
                whisper: "NotDownloaded",
                llm: "NotDownloaded",
                sortformer: "NotDownloaded",
            },
            openSettings,
        });

        render(<DemoModeBanner />);

        act(() => {
            fireEvent.click(
                screen.getByTestId("demo-banner-open-settings"),
            );
        });

        await waitFor(() => expect(openSettings).toHaveBeenCalledTimes(1));
    });
});
