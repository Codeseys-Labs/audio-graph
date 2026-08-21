import { describe, expect, it } from "vitest";
import {
  modeReadinessTone,
  readinessTone,
  selectabilityTone,
} from "./badgeTone";

/**
 * `badgeTone.ts` absorbed these pure helpers from `settings/Badge.tsx`
 * (audio-graph-4d67 — Badge absorption into `.ag-chip[data-tone]`). The
 * assertions below are carried over unchanged from `Badge.test.tsx`'s
 * "closed-variant fallback" describe block: the whole point of the typed
 * tone helpers (D3) is that an unknown status must NOT fall through to an
 * unstyled/undefined tone — it maps to `neutral`, which `.ag-chip`'s base
 * rule (no `data-tone`, or an unrecognized one) always renders styled.
 */
describe("badge tone helpers (closed-variant fallback, D3 open-set bug fix)", () => {
  it("maps known readiness statuses, neutral for unknown", () => {
    expect(readinessTone("ready")).toBe("success");
    expect(readinessTone("error")).toBe("danger");
    expect(readinessTone("missing_credentials")).toBe("warning");
    expect(readinessTone("unchecked")).toBe("warning");
    // A future / off-spec backend status — the old `--${status}` BEM class
    // would have been unstyled; now it is a styled neutral chip.
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
});
