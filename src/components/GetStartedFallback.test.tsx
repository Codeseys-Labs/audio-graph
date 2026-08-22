import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import GetStartedFallback from "./GetStartedFallback";

describe("GetStartedFallback", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("clicking 'Open settings' navigates bare (no MouseEvent forwarded as a route)", () => {
    // Settings T1 (seed audio-graph-2b9a): `onOpenSettings` is threaded from
    // the store's `openSettings(route?)`, whose signature widened to accept
    // an optional route. A bare `onClick={onOpenSettings}` would forward the
    // click's React MouseEvent as that route argument — the same hazard
    // fixed at NowStrip.tsx. Pin the call is invoked with no arguments.
    const onOpenSettings = vi.fn();
    render(
      <GetStartedFallback
        onPreviewSample={vi.fn()}
        onRetry={vi.fn()}
        onOpenSettings={onOpenSettings}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /open settings/i }));
    expect(onOpenSettings).toHaveBeenCalledWith();
  });
});
