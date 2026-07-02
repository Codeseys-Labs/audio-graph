import { fireEvent, render, screen, within } from "@testing-library/react";
import i18n from "i18next";
import { describe, expect, it, vi } from "vitest";
import "../../i18n";
import type {
  CredentialPresence,
  ModelInfo,
  ProviderReadiness,
} from "../../types";
import type { SettingsControllerValue } from "./useSettingsController";

// CredentialsPanel is a thin presentational view over `useSettings()`. We mock
// the context with a minimal controllable value (mirrors settingsRail.test.tsx)
// so this test isolates the readiness card's Download/Delete wiring from the
// heavyweight controller (which pulls in Tauri invoke). The provider→model
// join uses the REAL generated registry (PROVIDER_DESCRIPTORS) so the local
// vs cloud gating is exercised end-to-end.
const mockUseSettings = vi.fn();
vi.mock("./SettingsContext", () => ({
  useSettings: () => mockUseSettings(),
}));

import CredentialsPanel from "./CredentialsPanel";

const t = i18n.getFixedT("en");

// `asr.local_whisper` requires the single file `ggml-small.en.bin`
// (src/generated/providerRegistry.ts). The catalog `ModelInfo.filename` uses
// the same constant, which is the join key.
const WHISPER_SMALL = "ggml-small.en.bin";

function whisperModel(overrides: Partial<ModelInfo> = {}): ModelInfo {
  return {
    name: "Whisper Small (English)",
    filename: WHISPER_SMALL,
    url: "",
    size_bytes: 487_654_400,
    is_downloaded: false,
    is_valid: false,
    description: "desc-small",
    local_path: null,
    ...overrides,
  };
}

function readiness(
  overrides: Partial<ProviderReadiness> = {},
): ProviderReadiness {
  return {
    provider_id: "asr.local_whisper",
    status: "unchecked",
    message: "msg",
    stale: false,
    credential_epoch: 0,
    credentials: [],
    ...overrides,
  };
}

function makeValue(
  overrides: Partial<SettingsControllerValue> = {},
): SettingsControllerValue {
  return {
    t,
    savedCredentialEntries: [],
    relatedReadinessForCredential: () => [],
    providerLabelsForCredential: () => [],
    latestValidationForCredential: () => null,
    credentialRouteForKey: () => null,
    refreshProviderReadiness: vi.fn(),
    providerReadinessLoading: false,
    handleClearCredential: vi.fn(),
    providerReadinessError: null,
    providerReadinessStatusSummary: "",
    visibleProviderReadiness: [] as ProviderReadiness[],
    activeReadinessProviderIdSet: new Set<string>(),
    selectedModelForProvider: () => null,
    credentialRouteForReadiness: () => null,
    credentialPresence: {},
    handleOpenCredentialRoute: vi.fn(),
    credentialDrafts: {} as Record<string, string>,
    credentialSaveNotice: {} as Record<string, string>,
    setCredentialDraft: vi.fn(),
    handleSaveCredentialValue: vi.fn(),
    models: [] as ModelInfo[],
    downloadModel: vi.fn(),
    handleDeleteClick: vi.fn(),
    confirmDelete: null,
    downloadProgress: null,
    isDownloading: false,
    isDeletingModel: null,
    ...overrides,
  } as unknown as SettingsControllerValue;
}

describe("CredentialsPanel readiness model actions", () => {
  it("renders a Download button that downloads the correct filename when the local model is not on disk", () => {
    const downloadModel = vi.fn();
    mockUseSettings.mockReturnValue(
      makeValue({
        visibleProviderReadiness: [readiness()],
        models: [whisperModel({ is_downloaded: false })],
        downloadModel,
      }),
    );
    render(<CredentialsPanel />);

    const row = screen.getByTestId(`readiness-model-${WHISPER_SMALL}`);
    const download = within(row).getByRole("button", { name: /download/i });
    expect(download).toBeInTheDocument();
    // No delete affordance while the model is missing.
    expect(
      within(row).queryByRole("button", { name: /delete/i }),
    ).not.toBeInTheDocument();

    fireEvent.click(download);
    expect(downloadModel).toHaveBeenCalledTimes(1);
    expect(downloadModel).toHaveBeenCalledWith(WHISPER_SMALL);
  });

  it("renders a Delete button that arms the correct filename when the local model is downloaded", () => {
    const handleDeleteClick = vi.fn();
    mockUseSettings.mockReturnValue(
      makeValue({
        visibleProviderReadiness: [readiness({ status: "ready" })],
        models: [whisperModel({ is_downloaded: true, is_valid: true })],
        handleDeleteClick,
      }),
    );
    render(<CredentialsPanel />);

    const row = screen.getByTestId(`readiness-model-${WHISPER_SMALL}`);
    const del = within(row).getByRole("button", { name: /delete/i });
    expect(del).toBeInTheDocument();
    // No download affordance once the model is present.
    expect(
      within(row).queryByRole("button", { name: /^download$/i }),
    ).not.toBeInTheDocument();

    fireEvent.click(del);
    expect(handleDeleteClick).toHaveBeenCalledTimes(1);
    expect(handleDeleteClick).toHaveBeenCalledWith(WHISPER_SMALL);
  });

  it("renders NEITHER Download nor Delete for a cloud provider readiness card", () => {
    mockUseSettings.mockReturnValue(
      makeValue({
        // Deepgram is a cloud provider — empty `local_models`, so no join rows.
        visibleProviderReadiness: [
          readiness({ provider_id: "asr.deepgram", status: "ready" }),
        ],
        // Even if the whisper file is on disk, the cloud card must not surface it.
        models: [whisperModel({ is_downloaded: true, is_valid: true })],
      }),
    );
    render(<CredentialsPanel />);

    expect(
      screen.queryByTestId(`readiness-model-${WHISPER_SMALL}`),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /download/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /delete/i }),
    ).not.toBeInTheDocument();
  });
});

// ── In-place credential entry on the credential-health rows ─────────────────
// The rows used to be status-only: "Replace" navigated to the STT/LLM tab and
// the global footer Save silently sent "" when the field wasn't edited. Each
// routable row now carries the shared SecretCredentialControl + a Save button
// that drives `handleSaveCredentialValue`, and an empty save surfaces a visible
// notice instead of a silent no-op.
const deepgramCredential: CredentialPresence = {
  key: "deepgram_api_key",
  present: true,
  source: "credentials_yaml",
};

// A route object shaped like the real `credentialRouteForKey` result. Only its
// presence (non-null) is load-bearing — it gates the in-place editor.
const routeStub = {
  tab: "stt" as const,
  fieldId: "deepgram-api-key",
  activate: true,
};

describe("CredentialsPanel in-place credential entry", () => {
  it("saves the row's key in place via handleSaveCredentialValue", () => {
    const handleSaveCredentialValue = vi.fn();
    mockUseSettings.mockReturnValue(
      makeValue({
        savedCredentialEntries: [deepgramCredential],
        credentialRouteForKey: () => routeStub,
        credentialDrafts: { deepgram_api_key: "dg-typed" },
        handleSaveCredentialValue,
      }),
    );
    render(<CredentialsPanel />);

    const save = screen.getByRole("button", {
      name: t("settings.credentialHealth.save"),
    });
    fireEvent.click(save);

    expect(handleSaveCredentialValue).toHaveBeenCalledTimes(1);
    expect(handleSaveCredentialValue).toHaveBeenCalledWith("deepgram_api_key");
  });

  it("routes onChange edits through setCredentialDraft keyed by the row", () => {
    const setCredentialDraft = vi.fn();
    mockUseSettings.mockReturnValue(
      makeValue({
        savedCredentialEntries: [deepgramCredential],
        credentialRouteForKey: () => routeStub,
        // A non-empty draft forces SecretCredentialControl to show its input.
        credentialDrafts: { deepgram_api_key: "d" },
        setCredentialDraft,
      }),
    );
    render(<CredentialsPanel />);

    const input = screen.getByLabelText("deepgram_api_key");
    fireEvent.change(input, { target: { value: "dg-new" } });

    expect(setCredentialDraft).toHaveBeenCalledWith(
      "deepgram_api_key",
      "dg-new",
    );
  });

  it("surfaces the visible empty-save notice instead of a silent no-op", () => {
    mockUseSettings.mockReturnValue(
      makeValue({
        savedCredentialEntries: [deepgramCredential],
        credentialRouteForKey: () => routeStub,
        credentialSaveNotice: { deepgram_api_key: "empty" },
      }),
    );
    render(<CredentialsPanel />);

    expect(
      screen.getByText(t("settings.credentialHealth.saveNotice.empty")),
    ).toBeInTheDocument();
  });

  it("does NOT render the in-place editor for a non-routable key", () => {
    mockUseSettings.mockReturnValue(
      makeValue({
        savedCredentialEntries: [
          { key: "mystery_key", present: true, source: "credentials_yaml" },
        ],
        credentialRouteForKey: () => null,
      }),
    );
    render(<CredentialsPanel />);

    // The Save-key affordance only appears for routable rows.
    expect(
      screen.queryByRole("button", {
        name: t("settings.credentialHealth.save"),
      }),
    ).not.toBeInTheDocument();
  });
});
