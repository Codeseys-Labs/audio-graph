import { render, screen, within } from "@testing-library/react";
import i18n from "i18next";
import { describe, expect, it, vi } from "vitest";
import "../../i18n";
import type { ProviderSetupModeCard } from "../providerSetupModes";
import type { SettingsControllerValue } from "./useSettingsController";

// Isolates ProductModeSummaryCards from the heavyweight controller (mirrors
// CredentialsPanel.test.tsx) so the mode-card readiness chip's tone LAW
// (audio-graph-2554, settings T2) can be exercised with a precise,
// hand-built `ProviderSetupModeCard` fixture instead of the full provider
// registry + settings store.
const mockUseSettings = vi.fn();
vi.mock("./SettingsContext", () => ({
  useSettings: () => mockUseSettings(),
}));

import ProductModeSummaryCards from "./ProductModeSummaryCards";

const t = i18n.getFixedT("en");

function card(
  overrides: Partial<ProviderSetupModeCard>,
): ProviderSetupModeCard {
  return {
    id: "cloud_fast",
    label: "Cloud fast",
    description: "",
    productPath: "durable_notes_graph",
    selected: false,
    uiSelectable: true,
    selectedProviders: [],
    stageCoverage: [],
    dataBoundary: "vendor_cloud",
    dataLeavesDevice: true,
    readinessStatus: "ready",
    missingBlockers: [],
    ...overrides,
  };
}

function makeValue(cards: ProviderSetupModeCard[]): SettingsControllerValue {
  return {
    providerSetupModeCards: cards,
    providerSetupProviderRoute: () => null,
    providerSetupCredentialRoute: () => null,
    providerSetupModelRoute: () => null,
    providerRouteForProviderId: () => null,
    openSettingsControlRoute: vi.fn(),
    handleProviderSetupSourceRecovery: vi.fn(),
    handleSelectProductMode: vi.fn(),
  } as unknown as SettingsControllerValue;
}

describe("ProductModeSummaryCards — the tone law (audio-graph-2554, settings T2)", () => {
  it("shows Ready + success for the mode that IS actually selected/active", () => {
    mockUseSettings.mockReturnValue(
      makeValue([
        card({ id: "cloud_fast", selected: true, readinessStatus: "ready" }),
      ]),
    );
    render(<ProductModeSummaryCards />);

    const heading = screen.getByRole("heading", { name: "Cloud fast" });
    const cardEl = heading.closest(".settings-mode-card") as HTMLElement;
    const chip = within(cardEl).getByText(
      t("settings.providerReadiness.status.ready"),
    );
    expect(chip).toHaveAttribute("data-tone", "success");
  });

  it("renders no axis-3 chip for a non-selected mode candidate, even when its aggregate status says ready", () => {
    // A non-selected candidate mode's providers are not the ones actually
    // running — a "ready" aggregate here would be a claim about a mode
    // nobody switched to (the same law as the render sites' Soniox case).
    mockUseSettings.mockReturnValue(
      makeValue([
        card({ id: "cloud_fast", selected: true, readinessStatus: "ready" }),
        card({
          id: "local_private",
          label: "Local private",
          selected: false,
          readinessStatus: "ready",
        }),
      ]),
    );
    render(<ProductModeSummaryCards />);

    const heading = screen.getByRole("heading", { name: "Local private" });
    const cardEl = heading.closest(".settings-mode-card") as HTMLElement;
    const badges = cardEl.querySelector(
      ".settings-mode-card__badges",
    ) as HTMLElement;
    expect(within(badges).queryByText("Ready")).not.toBeInTheDocument();
    expect(badges.querySelectorAll(".ag-chip")).toHaveLength(0);
  });

  it("still renders a real missing-key/error status for a non-selected mode (only the ready claim is gated)", () => {
    mockUseSettings.mockReturnValue(
      makeValue([
        card({
          id: "cloud_fast",
          label: "Cloud fast",
          selected: false,
          readinessStatus: "missing_credentials",
        }),
      ]),
    );
    render(<ProductModeSummaryCards />);

    const heading = screen.getByRole("heading", { name: "Cloud fast" });
    const cardEl = heading.closest(".settings-mode-card") as HTMLElement;
    const chip = within(cardEl).getByText(
      t("settings.providerReadiness.status.missing_credentials"),
    );
    expect(chip).toHaveAttribute("data-tone", "warning");
  });
});
