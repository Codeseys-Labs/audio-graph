import { render, screen, within } from "@testing-library/react";
import i18n from "i18next";
import { describe, expect, it, vi } from "vitest";
import "../../i18n";
import { GENERATED_PROVIDER_REGISTRY } from "../../generated/providerRegistry";
import type { SettingsControllerValue } from "./useSettingsController";

// Isolates ProviderCapabilityCard from the heavyweight controller (mirrors
// CredentialsPanel.test.tsx) so the readiness-badge tone LAW (audio-graph-2554,
// settings T2) can be exercised with precise, controlled readiness fixtures.
const mockUseSettings = vi.fn();
vi.mock("./SettingsContext", () => ({
  useSettings: () => mockUseSettings(),
}));

import ProviderCapabilityCard from "./ProviderCapabilityCard";

const t = i18n.getFixedT("en");

const SONIOX = GENERATED_PROVIDER_REGISTRY.find(
  (provider) => provider.id === "asr.soniox",
);
const DEEPGRAM = GENERATED_PROVIDER_REGISTRY.find(
  (provider) => provider.id === "asr.deepgram",
);
if (!SONIOX || !DEEPGRAM) {
  throw new Error("Expected asr.soniox and asr.deepgram in the registry");
}

function makeValue(
  overrides: Partial<SettingsControllerValue> = {},
): SettingsControllerValue {
  return {
    t,
    providerReadiness: {},
    providerRouteForProviderId: () => null,
    activeReadinessProviderIdSet: new Set<string>(),
    credentialPresence: {},
    openSettingsControlRoute: vi.fn(),
    ...overrides,
  } as unknown as SettingsControllerValue;
}

describe("ProviderCapabilityCard — the tone law (audio-graph-2554, settings T2)", () => {
  it("renders NO axis-3 readiness chip for a non-active provider, even with a real 'ready' backend probe", () => {
    // asr.soniox is `ui_selectable: false` (Planned) and, critically, is NOT
    // in `activeReadinessProviderIdSet` here — yet its readiness entry
    // genuinely reports "ready" (the key really does validate). Rendering
    // that as a success chip on a card nobody can select is exactly the
    // false claim the law forbids.
    mockUseSettings.mockReturnValue(
      makeValue({
        providerReadiness: {
          "asr.soniox": {
            provider_id: "asr.soniox",
            status: "ready",
            message: "Soniox key is valid but provider remains planned",
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "soniox_api_key", present: true }],
          },
        },
        activeReadinessProviderIdSet: new Set<string>(),
      }),
    );

    render(<ProviderCapabilityCard descriptor={SONIOX} stageLabel="ASR" />);

    const card = screen
      .getByText(SONIOX.display_name)
      .closest(".settings-provider-capability-card") as HTMLElement;
    const badges = card.querySelector(
      ".settings-provider-capability-card__badges",
    ) as HTMLElement;
    expect(within(badges).queryByText("Ready")).not.toBeInTheDocument();
    // Only the selectability badge ("Planned") remains — no axis-3 chip at
    // all, not even a demoted/neutral one.
    expect(badges.querySelectorAll(".ag-chip")).toHaveLength(1);
    // The raw technical dl dump (out of the badge's audited scope) is
    // untouched — it still echoes the backend's literal report for
    // inspection, so this is a deliberate scope boundary, not an oversight.
    const readinessRow = within(card)
      .getByText("Readiness")
      .closest("div") as HTMLElement;
    expect(readinessRow).toHaveTextContent("Ready");
  });

  it("routes the readiness badge through the helper: an active provider's error never renders success", () => {
    mockUseSettings.mockReturnValue(
      makeValue({
        providerReadiness: {
          "asr.deepgram": {
            provider_id: "asr.deepgram",
            status: "error",
            message: "Deepgram key failed validation",
            stale: false,
            credential_epoch: 0,
            credentials: [{ key: "deepgram_api_key", present: true }],
          },
        },
        activeReadinessProviderIdSet: new Set(["asr.deepgram"]),
      }),
    );

    render(<ProviderCapabilityCard descriptor={DEEPGRAM} stageLabel="ASR" />);

    const card = screen
      .getByText(DEEPGRAM.display_name)
      .closest(".settings-provider-capability-card") as HTMLElement;
    const badges = card.querySelector(
      ".settings-provider-capability-card__badges",
    ) as HTMLElement;
    // "Selected" (active/PLANNED-axis) coexists with a failing (OBSERVED-axis)
    // readiness chip — the two axes never blend into one green claim.
    const selectedChip = within(badges).getByText("Selected");
    expect(selectedChip).toHaveAttribute("data-tone", "accent");
    const readinessChip = within(badges).getByText("Error");
    expect(readinessChip).toHaveAttribute("data-tone", "danger");
    expect(within(badges).queryByText("Ready")).not.toBeInTheDocument();
  });

  it("demotes a stale ready to the existing 'Unchecked' copy instead of an unchanged success chip", () => {
    mockUseSettings.mockReturnValue(
      makeValue({
        providerReadiness: {
          "asr.deepgram": {
            provider_id: "asr.deepgram",
            status: "ready",
            message: "Deepgram key was valid as of the last check",
            stale: true,
            credential_epoch: 0,
            credentials: [{ key: "deepgram_api_key", present: true }],
          },
        },
        activeReadinessProviderIdSet: new Set(["asr.deepgram"]),
      }),
    );

    render(<ProviderCapabilityCard descriptor={DEEPGRAM} stageLabel="ASR" />);

    const card = screen
      .getByText(DEEPGRAM.display_name)
      .closest(".settings-provider-capability-card") as HTMLElement;
    const badges = card.querySelector(
      ".settings-provider-capability-card__badges",
    ) as HTMLElement;
    const readinessChip = within(badges).getByText(
      t("settings.providerReadiness.status.unchecked"),
    );
    expect(readinessChip).toHaveAttribute("data-tone", "neutral");
    expect(within(badges).queryByText("Ready")).not.toBeInTheDocument();
  });
});
