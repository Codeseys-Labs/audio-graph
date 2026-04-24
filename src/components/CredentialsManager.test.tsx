import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import i18n from "i18next";
import CredentialsManager from "./CredentialsManager";
import type { ModelInfo } from "../types";
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

function renderManager(modelsOverride?: ModelInfo[]) {
    const t = i18n.getFixedT("en");
    return render(
        <CredentialsManager
            state={{ confirmDelete: null, logLevel: "info" }}
            t={t}
            models={modelsOverride ?? whisperModels}
            modelStatus={null}
            isDownloading={false}
            isDeletingModel={null}
            downloadProgress={null}
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
