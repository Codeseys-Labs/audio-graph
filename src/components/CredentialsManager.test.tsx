import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import i18n from "i18next";
import CredentialsManager from "./CredentialsManager";
import type { DownloadProgress, ModelInfo } from "../types";
import "../i18n";

const whisperModels: ModelInfo[] = [
    {
        name: "Whisper Tiny (English)",
        filename: "ggml-tiny.en.bin",
        url: "",
        size_bytes: 77_700_000,
        is_downloaded: false,
        is_valid: false,
        description: "desc-tiny",
        local_path: null,
    },
    {
        name: "Whisper Base (English)",
        filename: "ggml-base.en.bin",
        url: "",
        size_bytes: 147_500_000,
        is_downloaded: false,
        is_valid: false,
        description: "desc-base",
        local_path: null,
    },
    {
        name: "Whisper Small (English)",
        filename: "ggml-small.en.bin",
        url: "",
        size_bytes: 487_654_400,
        is_downloaded: false,
        is_valid: false,
        description: "desc-small",
        local_path: null,
    },
    {
        name: "Whisper Medium (English)",
        filename: "ggml-medium.en.bin",
        url: "",
        size_bytes: 1_533_800_000,
        is_downloaded: false,
        is_valid: false,
        description: "desc-medium",
        local_path: null,
    },
    {
        name: "Whisper Large v3 (Multilingual)",
        filename: "ggml-large-v3.bin",
        url: "",
        size_bytes: 3_094_600_000,
        is_downloaded: false,
        is_valid: false,
        description: "desc-large",
        local_path: null,
    },
    {
        name: "LFM2-350M Extract (Entity Extraction)",
        filename: "lfm2-350m-extract-q4_k_m.gguf",
        url: "",
        size_bytes: 229_000_000,
        is_downloaded: false,
        is_valid: false,
        description: "desc-llm",
        local_path: null,
    },
];

function renderManager(
    modelsOverride?: ModelInfo[],
    opts?: {
        isDownloading?: boolean;
        downloadProgress?: DownloadProgress | null;
    },
) {
    const t = i18n.getFixedT("en");
    return render(
        <CredentialsManager
            state={{ confirmDelete: null, logLevel: "info" }}
            t={t}
            models={modelsOverride ?? whisperModels}
            modelStatus={null}
            isDownloading={opts?.isDownloading ?? false}
            isDeletingModel={null}
            downloadProgress={opts?.downloadProgress ?? null}
            downloadModel={vi.fn()}
            handleDeleteClick={vi.fn()}
            handleLogLevelChange={vi.fn(async () => {})}
        />,
    );
}

describe("CredentialsManager model guidance", () => {
    it("renders the correct guidance subtitle next to each Whisper tier and the local LLM", () => {
        renderManager();

        const expected: Array<[string, RegExp]> = [
            ["ggml-tiny.en.bin", /fastest, but noisy transcripts/i],
            ["ggml-base.en.bin", /recommended default/i],
            ["ggml-small.en.bin", /needs ~4 GB RAM/i],
            ["ggml-medium.en.bin", /best quality local option/i],
            ["ggml-large-v3.bin", /state of the art, requires GPU/i],
            ["lfm2-350m-extract-q4_k_m.gguf", /small local LLM/i],
        ];

        for (const [filename, textMatcher] of expected) {
            const hint = screen.getByTestId(`model-guidance-${filename}`);
            expect(hint).toBeInTheDocument();
            expect(hint).toHaveTextContent(textMatcher);
            // Guidance must render inside the model-card container it
            // describes, so users visually associate it with that model.
            expect(hint.closest(".model-card")).not.toBeNull();
        }
    });

    it("renders a downloaded/total + ETA line when a download-progress event arrives", () => {
        // Simulate the payload the backend emits ~1s into a download: 10 MB of
        // a 100 MB file in 2 seconds. ETA should be (90MB / (10MB/2s)) = 18s.
        const progress: DownloadProgress = {
            model_id: "ggml-base.en.bin",
            model_name: "Whisper Base (English)",
            bytes_downloaded: 10 * 1024 * 1024,
            total_bytes: 100 * 1024 * 1024,
            elapsed_ms: 2000,
            percent: 10,
            status: "downloading",
        };
        renderManager(undefined, {
            isDownloading: true,
            downloadProgress: progress,
        });

        const line = screen.getByTestId("model-progress-ggml-base.en.bin");
        expect(line).toBeInTheDocument();
        // Downloaded size, total size, and the word "remaining" must all be
        // present so the user sees progress + time estimate on one line.
        expect(line).toHaveTextContent(/10 MB/);
        expect(line).toHaveTextContent(/100 MB/);
        expect(line).toHaveTextContent(/18s remaining/);
    });

    it("falls back to bytes-only text when total_bytes is 0 (unknown size)", () => {
        // Content-Length missing from the server response: we encode as 0 and
        // the UI must avoid dividing by it (which would render NaN / Infinity).
        const progress: DownloadProgress = {
            model_id: "ggml-base.en.bin",
            model_name: "Whisper Base (English)",
            bytes_downloaded: 5 * 1024 * 1024,
            total_bytes: 0,
            elapsed_ms: 1000,
            percent: 0,
            status: "downloading",
        };
        renderManager(undefined, {
            isDownloading: true,
            downloadProgress: progress,
        });

        const line = screen.getByTestId("model-progress-ggml-base.en.bin");
        expect(line).toHaveTextContent(/5 MB downloaded/);
        expect(line).not.toHaveTextContent(/remaining/);
        expect(line).not.toHaveTextContent(/NaN|Infinity/);
    });

    it("hides the progress line once the event reports status=complete", () => {
        const progress: DownloadProgress = {
            model_id: "ggml-base.en.bin",
            model_name: "Whisper Base (English)",
            bytes_downloaded: 100 * 1024 * 1024,
            total_bytes: 100 * 1024 * 1024,
            elapsed_ms: 10_000,
            percent: 100,
            status: "complete",
        };
        renderManager(undefined, {
            isDownloading: false,
            downloadProgress: progress,
        });

        expect(
            screen.queryByTestId("model-progress-ggml-base.en.bin"),
        ).not.toBeInTheDocument();
    });

    it("omits the guidance subtitle for models that have no tier-based hint", () => {
        const unknownModel: ModelInfo = {
            name: "Sortformer v2 (Speaker Diarization)",
            filename: "diar_streaming_sortformer_4spk-v2.onnx",
            url: "",
            size_bytes: 31_500_000,
            is_downloaded: false,
            is_valid: false,
            description: "desc-sortformer",
            local_path: null,
        };

        renderManager([unknownModel]);

        expect(
            screen.queryByTestId(
                `model-guidance-${unknownModel.filename}`,
            ),
        ).not.toBeInTheDocument();
    });
});
