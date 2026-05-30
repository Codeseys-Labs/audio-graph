import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import Tooltip from "./Tooltip";

describe("Tooltip", () => {
  it("renders the trigger child as-is (asChild) and not the content by default", () => {
    render(
      <Tooltip content="Helpful hint">
        <button type="button">Trigger</button>
      </Tooltip>,
    );
    expect(screen.getByRole("button", { name: "Trigger" })).toBeInTheDocument();
    // Content is closed until hover/focus.
    expect(screen.queryByText("Helpful hint")).not.toBeInTheDocument();
  });

  it("reveals the content on keyboard focus of the trigger", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip content="Focus hint" delayDuration={0}>
        <button type="button">Focus me</button>
      </Tooltip>,
    );

    await user.tab();
    expect(screen.getByRole("button", { name: "Focus me" })).toHaveFocus();

    // Radix renders the content (often duplicated: visible + a11y mirror)
    // once the trigger is focused.
    await waitFor(() => {
      expect(screen.getAllByText("Focus hint").length).toBeGreaterThan(0);
    });
  });

  it("reveals the content on pointer hover of the trigger", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip content="Hover hint" delayDuration={0}>
        <button type="button">Hover me</button>
      </Tooltip>,
    );

    await user.hover(screen.getByRole("button", { name: "Hover me" }));

    await waitFor(() => {
      expect(screen.getAllByText("Hover hint").length).toBeGreaterThan(0);
    });
  });
});
