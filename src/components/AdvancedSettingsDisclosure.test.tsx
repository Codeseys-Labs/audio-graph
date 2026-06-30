import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import AdvancedSettingsDisclosure from "./AdvancedSettingsDisclosure";

describe("AdvancedSettingsDisclosure", () => {
  it("labels the disclosed body as a group named by the summary", () => {
    render(
      <AdvancedSettingsDisclosure summary="Advanced provider controls">
        <input aria-label="Tuning knob" />
      </AdvancedSettingsDisclosure>,
    );

    // The summary names the revealed group so a screen reader announces the
    // controls as "Advanced provider controls group" rather than a bare run.
    const group = screen.getByRole("group", {
      name: /advanced provider controls/i,
    });
    expect(group).toBeInTheDocument();
    const summary = screen.getByText("Advanced provider controls");
    expect(group).toHaveAttribute("aria-labelledby", summary.id);
    expect(summary.id).toBeTruthy();
  });

  it("gives each disclosure on a panel a distinct summary id", () => {
    render(
      <>
        <AdvancedSettingsDisclosure summary="First group">
          <span>one</span>
        </AdvancedSettingsDisclosure>
        <AdvancedSettingsDisclosure summary="Second group">
          <span>two</span>
        </AdvancedSettingsDisclosure>
      </>,
    );

    const first = screen.getByText("First group");
    const second = screen.getByText("Second group");
    expect(first.id).toBeTruthy();
    expect(second.id).toBeTruthy();
    expect(first.id).not.toBe(second.id);
  });

  it("toggles open via the native summary affordance from the keyboard", () => {
    render(
      <AdvancedSettingsDisclosure summary="Advanced provider controls">
        <span>revealed body</span>
      </AdvancedSettingsDisclosure>,
    );

    const details = screen
      .getByText("Advanced provider controls")
      .closest("details") as HTMLDetailsElement;
    expect(details.open).toBe(false);

    // Native <details>/<summary> handles Enter/Space toggling; jsdom drives the
    // open state through the summary's click activation, which is exactly what
    // a keyboard Enter/Space produces on a focused summary.
    const summary = screen.getByText("Advanced provider controls");
    summary.focus();
    expect(summary).toHaveFocus();
    fireEvent.click(summary);
    expect(details.open).toBe(true);

    fireEvent.click(summary);
    expect(details.open).toBe(false);
  });
});
