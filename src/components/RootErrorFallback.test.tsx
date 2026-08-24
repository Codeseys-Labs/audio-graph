import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import RootErrorFallback from "./RootErrorFallback";

describe("RootErrorFallback", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("shows the error class only — never a message or stack (SECURITY)", () => {
    render(
      <RootErrorFallback
        errorName="TypeError"
        onReload={vi.fn()}
        onBackToCapture={vi.fn()}
      />,
    );
    expect(screen.getByTestId("root-error-fallback-class")).toHaveTextContent(
      "TypeError",
    );
    // Nothing rendered anywhere carries stack-shaped text (a raw stack would
    // include a "at " frame line and/or a file path).
    expect(document.body.textContent).not.toMatch(/\bat .*:\d+:\d+/);
  });

  it("calls onReload bare (no MouseEvent forwarded) when Reload is clicked", () => {
    const onReload = vi.fn();
    render(
      <RootErrorFallback
        errorName="TypeError"
        onReload={onReload}
        onBackToCapture={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /reload/i }));
    expect(onReload).toHaveBeenCalledTimes(1);
    expect(onReload).toHaveBeenCalledWith();
  });

  it("calls onBackToCapture bare (no MouseEvent forwarded) when Back to Capture is clicked", () => {
    const onBackToCapture = vi.fn();
    render(
      <RootErrorFallback
        errorName="RangeError"
        onReload={vi.fn()}
        onBackToCapture={onBackToCapture}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /back to capture/i }));
    expect(onBackToCapture).toHaveBeenCalledTimes(1);
    expect(onBackToCapture).toHaveBeenCalledWith();
  });

  it("localizes in Portuguese", async () => {
    await i18n.changeLanguage("pt");
    render(
      <RootErrorFallback
        errorName="TypeError"
        onReload={vi.fn()}
        onBackToCapture={vi.fn()}
      />,
    );
    expect(screen.getByText(/algo deu errado/i)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /recarregar o audiograph/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /voltar para captura/i }),
    ).toBeInTheDocument();
  });
});
