import { act, fireEvent, render, screen } from "@testing-library/react";
import { useEffect, useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ShortcutsHelpModal from "./ShortcutsHelpModal";
import "../i18n";

// Must mirror HANDOFF_SEEN_KEY in App.tsx and ONBOARDING_HANDOFF_SEEN_KEY in
// ShortcutsHelpModal.tsx — the help modal coordinates with App's onboarding
// gate by this key name only.
const HANDOFF_SEEN_KEY = "ag.onboardingHandoffSeen";

// Minimal harness that mirrors the Cmd/Ctrl+/ + "?" binding in App.tsx, so we
// can exercise the open/close flow without dragging in all of App.tsx (which
// depends on ResizeObserver, the Zustand store, Tauri event plumbing, etc.).
function Harness() {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const typing =
        !!target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable);
      if (typing) return;
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key === "/") {
        e.preventDefault();
        setOpen((o) => !o);
      } else if (!mod && e.key === "?") {
        e.preventDefault();
        setOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);
  return open ? <ShortcutsHelpModal onClose={() => setOpen(false)} /> : null;
}

describe("ShortcutsHelpModal", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("opens when Cmd+/ is pressed and lists at least one shortcut", () => {
    render(<Harness />);

    expect(
      screen.queryByRole("dialog", { name: /keyboard shortcuts/i }),
    ).not.toBeInTheDocument();

    act(() => {
      fireEvent.keyDown(window, { key: "/", metaKey: true });
    });

    expect(
      screen.getByRole("dialog", { name: /keyboard shortcuts/i }),
    ).toBeInTheDocument();

    // At least one shortcut description renders (the binding exists in
    // useKeyboardShortcuts.ts — if this ever drifts, update the list in
    // ShortcutsHelpModal.tsx).
    expect(
      screen.getByText(/start or stop audio capture/i),
    ).toBeInTheDocument();
  });

  it("closes on Escape", () => {
    render(<Harness />);

    act(() => {
      fireEvent.keyDown(window, { key: "/", ctrlKey: true });
    });
    expect(
      screen.getByRole("dialog", { name: /keyboard shortcuts/i }),
    ).toBeInTheDocument();

    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(
      screen.queryByRole("dialog", { name: /keyboard shortcuts/i }),
    ).not.toBeInTheDocument();
  });

  it("closes when the close button is clicked", () => {
    render(<Harness />);

    act(() => {
      fireEvent.keyDown(window, { key: "?" });
    });
    expect(
      screen.getByRole("dialog", { name: /keyboard shortcuts/i }),
    ).toBeInTheDocument();

    act(() => {
      fireEvent.click(
        screen.getByRole("button", { name: /close shortcuts help/i }),
      );
    });
    expect(
      screen.queryByRole("dialog", { name: /keyboard shortcuts/i }),
    ).not.toBeInTheDocument();
  });

  it("closes on backdrop click but NOT on dialog body click", () => {
    const onClose = vi.fn();
    render(<ShortcutsHelpModal onClose={onClose} />);
    const dialog = screen.getByRole("dialog", {
      name: /keyboard shortcuts/i,
    });
    const overlay = dialog.parentElement as HTMLElement;
    expect(overlay).toHaveClass("settings-overlay");

    // Clicking inside the modal body should NOT close (propagation is
    // stopped by the onClick guard).
    fireEvent.click(dialog);
    expect(onClose).not.toHaveBeenCalled();

    // Clicking the overlay itself should close.
    fireEvent.click(overlay);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("re-arms the onboarding hand-off by clearing the show-once key", () => {
    // Simulate a user who has already dismissed the getting-started nudge.
    localStorage.setItem(HANDOFF_SEEN_KEY, "1");

    render(<ShortcutsHelpModal onClose={vi.fn()} />);

    // No confirmation visible until the button is pressed.
    expect(
      screen.queryByText(/getting-started guide will appear again/i),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: /show getting-started guide again/i }),
    );

    // The persisted flag is cleared so App.tsx will surface the nudge again,
    // and an inline confirmation appears.
    expect(localStorage.getItem(HANDOFF_SEEN_KEY)).toBeNull();
    expect(
      screen.getByText(/getting-started guide will appear again/i),
    ).toBeInTheDocument();
  });
});
