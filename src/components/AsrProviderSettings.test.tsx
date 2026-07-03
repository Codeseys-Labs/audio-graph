import { describe, expect, it } from "vitest";
import { snapDeepgramModelAlias } from "./AsrProviderSettings";

describe("snapDeepgramModelAlias", () => {
  it("snaps a bare `flux` (any case / whitespace) to flux-general-en", () => {
    // Deepgram markets the model as "flux" but v2/listen rejects the bare id
    // with a 400 — the UI must snap it to the canonical English variant so it
    // is never committed verbatim (FIX-1 frontend guard).
    for (const alias of ["flux", "FLUX", "  Flux  ", "fLuX"]) {
      expect(snapDeepgramModelAlias(alias)).toBe("flux-general-en");
    }
  });

  it("leaves already-canonical flux ids untouched", () => {
    expect(snapDeepgramModelAlias("flux-general-en")).toBe("flux-general-en");
    expect(snapDeepgramModelAlias("flux-general-multi")).toBe(
      "flux-general-multi",
    );
  });

  it("leaves unrelated / valid model ids untouched", () => {
    for (const model of ["nova-3", "nova-3-general", "nova-2", ""]) {
      expect(snapDeepgramModelAlias(model)).toBe(model);
    }
  });
});
