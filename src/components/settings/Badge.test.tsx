import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import Badge, {
  type BadgeTone,
  modeReadinessTone,
  readinessTone,
  selectabilityTone,
} from "./Badge";

describe("Badge", () => {
  it("renders its children as text content", () => {
    render(<Badge tone="success">Ready</Badge>);
    expect(screen.getByText("Ready")).toBeInTheDocument();
  });

  it("applies the tint pair for each tone", () => {
    const cases: Array<[BadgeTone, string, string]> = [
      ["success", "bg-(--tint-success)", "text-(--text-on-tint-success)"],
      ["warning", "bg-(--tint-warning)", "text-(--text-on-tint-warning)"],
      ["danger", "bg-(--tint-danger)", "text-(--text-on-tint-danger)"],
      ["neutral", "bg-(--hover-overlay)", "text-text-muted"],
      ["accent", "bg-accent", "text-(--on-accent)"],
    ];
    for (const [tone, bg, fg] of cases) {
      const { unmount } = render(<Badge tone={tone}>{tone}</Badge>);
      const node = screen.getByText(tone);
      expect(node).toHaveClass(bg);
      expect(node).toHaveClass(fg);
      // Always carries the shared frame so it is never an unstyled span.
      expect(node).toHaveClass("rounded-sm");
      unmount();
    }
  });

  // The whole point of the typed Badge (D3): an unknown status must NOT render
  // an unstyled badge. The status→tone helpers fall back to a styled neutral.
  describe("closed-variant fallback (D3 open-set bug fix)", () => {
    it("maps known readiness statuses, neutral for unknown", () => {
      expect(readinessTone("ready")).toBe("success");
      expect(readinessTone("error")).toBe("danger");
      expect(readinessTone("missing_credentials")).toBe("warning");
      expect(readinessTone("unchecked")).toBe("warning");
      // A future / off-spec backend status — the old `--${status}` BEM class
      // would have been unstyled; now it is a styled neutral badge.
      expect(readinessTone("some_new_backend_status")).toBe("neutral");
    });

    it("maps mode-readiness statuses, neutral for unknown", () => {
      expect(modeReadinessTone("ready")).toBe("success");
      expect(modeReadinessTone("blocked")).toBe("warning");
      expect(modeReadinessTone("error")).toBe("danger");
      expect(modeReadinessTone("totally_unknown")).toBe("neutral");
    });

    it("maps selectability statuses, neutral for unknown", () => {
      expect(selectabilityTone("selectable")).toBe("success");
      expect(selectabilityTone("planned")).toBe("warning");
      expect(selectabilityTone("error")).toBe("danger");
      expect(selectabilityTone("???")).toBe("neutral");
    });

    it("renders a styled (non-blank) badge for an unknown status", () => {
      render(<Badge tone={readinessTone("brand_new_status")}>Mystery</Badge>);
      const node = screen.getByText("Mystery");
      // Styled, not unstyled: carries both the frame and a tone background.
      expect(node).toHaveClass("rounded-sm");
      expect(node).toHaveClass("bg-(--hover-overlay)");
    });
  });
});
