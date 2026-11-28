import { render, screen, within } from "@testing-library/react";
import i18n from "i18next";
import { describe, expect, it, vi } from "vitest";
import "../../i18n";
import { GENERATED_PROVIDER_REGISTRY } from "../../generated/providerRegistry";
import {
  FAILURE_CASES,
  failedReadiness,
  READINESS_NOTE_CASES,
  runtimeFailure,
  STALE_PASS,
} from "../../test/readinessFixtures";
import type { ProviderReadiness } from "../../types";
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
            note: { kind: "verified" },
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
    // Tone-law completion (settings T3, audio-graph-9d2b — folds in seed
    // 73bf): the "raw technical dl dump stays a literal echo for inspection"
    // scope boundary this test used to lock in is exactly the gap seed 73bf
    // found — ADR-0030 doesn't carve out an exception for "it's inside Show
    // advanced". The dl row now reads through the SAME law as the badge
    // above it, so the two can never disagree.
    const readinessRow = within(card)
      .getByText("Readiness")
      .closest("div") as HTMLElement;
    expect(readinessRow).not.toHaveTextContent("Ready");
    expect(readinessRow).toHaveTextContent("Unchecked");
  });

  it("routes the readiness badge through the helper: an active provider's error never renders success", () => {
    mockUseSettings.mockReturnValue(
      makeValue({
        providerReadiness: {
          "asr.deepgram": {
            provider_id: "asr.deepgram",
            ...failedReadiness(runtimeFailure("internal")),
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
            note: { kind: "verified" },
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

describe("ProviderCapabilityCard — readiness copy from closed fields", () => {
  const LOCALES = ["en", "pt"] as const;
  const renderEntry = (
    locale: (typeof LOCALES)[number],
    entry: ProviderReadiness | null,
  ) => {
    mockUseSettings.mockReturnValue(
      makeValue({
        t: i18n.getFixedT(locale),
        providerReadiness: entry ? { "asr.deepgram": entry } : {},
      }),
    );
    const view = render(
      <ProviderCapabilityCard descriptor={DEEPGRAM} stageLabel="ASR" />,
    );
    const card = view.container.querySelector(
      ".settings-provider-capability-card",
    ) as HTMLElement;
    return {
      view,
      card,
      summary: card.querySelector(
        ".settings-provider-capability-card__readiness",
      ) as HTMLElement,
    };
  };
  const makeEntry = (
    overrides: Partial<ProviderReadiness>,
  ): ProviderReadiness => ({
    provider_id: "asr.deepgram",
    status: "unchecked",
    note: { kind: "ready_to_check" },
    stale: false,
    credential_epoch: 0,
    credentials: [{ key: "deepgram_api_key", present: true }],
    ...overrides,
  });

  it.each(
    READINESS_NOTE_CASES.map((c) => [JSON.stringify(c.note), c] as const),
  )("renders the note %s as localized copy, not a raw token", (_label, c) => {
    for (const locale of LOCALES) {
      const { view, card, summary } = renderEntry(
        locale,
        makeEntry({ note: c.note }),
      );
      expect(summary.textContent).toBe(c[locale]);
      expect(card.textContent).not.toMatch(/runtime\/|credential\//);
      if (c.note.kind.includes("_")) {
        expect(card.textContent).not.toContain(c.note.kind);
      }
      view.unmount();
    }
  });

  it.each(
    FAILURE_CASES.map((c) => [c.name, c] as const),
  )("presents %s from its typed cause", (_name, c) => {
    for (const locale of LOCALES) {
      const { view, card, summary } = renderEntry(
        locale,
        makeEntry({ ...failedReadiness(c.diagnostic) }),
      );
      expect(summary.textContent).toBe(c.message[locale]);
      expect(card.textContent).not.toMatch(/runtime\/|credential\//);
      view.unmount();
    }
  });

  it("never words a stale passed check as a plain pass", () => {
    for (const locale of LOCALES) {
      const { view, summary } = renderEntry(
        locale,
        makeEntry(STALE_PASS.readiness),
      );
      expect(summary.textContent).toBe(STALE_PASS.message[locale]);
      view.unmount();
    }
  });

  it("keeps the no-readiness fallback localized", () => {
    for (const locale of LOCALES) {
      const { view, summary } = renderEntry(locale, null);
      expect(summary.textContent).toBe(
        i18n.t("settings.providerReadiness.noReadiness", { lng: locale }),
      );
      view.unmount();
    }
    expect(
      i18n.t("settings.providerReadiness.noReadiness", { lng: "pt" }),
    ).not.toBe(i18n.t("settings.providerReadiness.noReadiness", { lng: "en" }));
  });

  it("renders the local runtime row from its closed status and registry model id", () => {
    const { card, view } = renderEntry(
      "en",
      makeEntry({
        runtime: {
          status: "model_missing",
          model_id: "moonshine-small-streaming-en",
          runtime_version: "1.2.3",
        },
      }),
    );
    const runtimeRow = within(card).getByText("Runtime").closest("div");
    expect(runtimeRow).toHaveTextContent(
      "Model missing: The local model files for moonshine-small-streaming-en are missing or incomplete. Runtime version 1.2.3.",
    );
    view.unmount();

    const { card: ptCard } = renderEntry(
      "pt",
      makeEntry({
        runtime: {
          status: "model_missing",
          model_id: "moonshine-small-streaming-en",
        },
      }),
    );
    expect(
      within(ptCard).getByText("Runtime").closest("div"),
    ).toHaveTextContent(
      "Os arquivos do modelo local moonshine-small-streaming-en estão ausentes ou incompletos.",
    );
  });
});
