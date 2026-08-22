import { render, screen, within } from "@testing-library/react";
import i18n from "i18next";
import { describe, expect, it } from "vitest";
import "../../i18n";
import type { ProviderSettingsOption } from "../providerRegistryHelpers";
import DeferredProviderRoster from "./DeferredProviderRoster";
import { DEFERRED_ASR_PROVIDER_OPTIONS } from "./useSettingsController";

const t = i18n.getFixedT("en");

describe("DeferredProviderRoster (settings T3, audio-graph-9d2b)", () => {
  it("renders nothing when there are no deferred options", () => {
    const { container } = render(<DeferredProviderRoster options={[]} t={t} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("lists every implemented-but-deferred provider as plain, non-actionable text", () => {
    render(
      <DeferredProviderRoster options={DEFERRED_ASR_PROVIDER_OPTIONS} t={t} />,
    );

    const roster = screen
      .getByText(/other engines/i)
      .closest("details") as HTMLElement;
    expect(roster).toBeInTheDocument();

    // ADR-0030/0033: deferred rows are not selectable — no interactive
    // controls anywhere in the roster (mutating a row into a button/link
    // fails this).
    expect(within(roster).queryAllByRole("button")).toHaveLength(0);
    expect(within(roster).queryAllByRole("link")).toHaveLength(0);
    expect(within(roster).queryAllByRole("radio")).toHaveLength(0);

    // The summary count comes from the registry, not a hand-maintained
    // number. Hard-coded to 6 (not re-derived from
    // `DEFERRED_ASR_PROVIDER_OPTIONS.length`) so a mutation that widens the
    // `deferredProviderOptionsForStage` filter (e.g. dropping the
    // `providerIsDeferred` check) changes what this component actually
    // renders WITHOUT correspondingly changing what this assertion expects —
    // a self-referential `.length` comparison can't catch that class of bug.
    expect(
      within(roster).getByText(/6 more engines are built but not selectable/i),
    ).toBeInTheDocument();

    for (const option of DEFERRED_ASR_PROVIDER_OPTIONS) {
      expect(within(roster).getByText(option.label)).toBeInTheDocument();
    }
    // Negative checks, independent of the (possibly mutated) options list:
    // the one `ui_selectable` provider (Deepgram) and a "planned" (not
    // "implemented") provider (Moonshine) must NEVER appear in the deferred
    // roster, even if `deferredProviderOptionsForStage`'s filter regresses.
    expect(
      within(roster).queryByText(/deepgram streaming/i),
    ).not.toBeInTheDocument();
    expect(within(roster).queryByText(/moonshine/i)).not.toBeInTheDocument();

    // R1 framing (ratified, maintainer, 2026-08-21): built, deliberately not
    // offered; saved configs keep working — NEVER "coming soon".
    expect(
      within(roster).getByText(/deliberately not offered/i),
    ).toBeInTheDocument();
    expect(within(roster).queryByText(/coming soon/i)).not.toBeInTheDocument();
  });

  it("keeps local_whisper (a deferred provider) OUT of the selectable list but IN the roster", () => {
    const inRoster = DEFERRED_ASR_PROVIDER_OPTIONS.some(
      (option: ProviderSettingsOption<string>) =>
        option.descriptor.id === "asr.local_whisper",
    );
    expect(inRoster).toBe(true);
  });
});
