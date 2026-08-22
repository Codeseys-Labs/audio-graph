import { describe, expect, it } from "vitest";
import type { ProviderDescriptor, ProviderReadiness } from "../../types";
import { railEngineInfoForProvider } from "./railEngineInfo";

function descriptor(display_name: string): ProviderDescriptor {
  return { display_name } as ProviderDescriptor;
}

function readiness(
  overrides: Partial<ProviderReadiness> = {},
): ProviderReadiness {
  return {
    provider_id: "test.provider",
    status: "ready",
    message: "",
    stale: false,
    credential_epoch: 0,
    credentials: [],
    ...overrides,
  } as ProviderReadiness;
}

describe("railEngineInfoForProvider — T4a rail engine line", () => {
  it("surfaces the provider's own display_name unlocalized (matches CredentialsPanel's existing precedent)", () => {
    const info = railEngineInfoForProvider(
      descriptor("Cerebras"),
      readiness({ status: "ready" }),
      true,
    );
    expect(info.providerLabel).toBe("Cerebras");
  });

  it("null descriptor resolves to a null providerLabel rather than throwing", () => {
    const info = railEngineInfoForProvider(null, readiness(), true);
    expect(info.providerLabel).toBeNull();
  });

  it("routes the chip through T2's law: a fresh active ready renders success", () => {
    const info = railEngineInfoForProvider(
      descriptor("Deepgram streaming"),
      readiness({ status: "ready", stale: false }),
      true,
    );
    expect(info.chipRender).toBe(true);
    expect(info.chipTone).toBe("success");
    expect(info.chipEffectiveStatus).toBe("ready");
  });

  it("a non-active provider's cached ready renders NO chip at all (ADR-0030 — presence-only rows never earn Ready)", () => {
    const info = railEngineInfoForProvider(
      descriptor("Sherpa-ONNX streaming"),
      readiness({ status: "ready", stale: false }),
      false,
    );
    expect(info.chipRender).toBe(false);
  });

  it("stale demotes the chip to neutral/unchecked, never a leaked success", () => {
    const info = railEngineInfoForProvider(
      descriptor("Cerebras"),
      readiness({ status: "ready", stale: true }),
      true,
    );
    expect(info.chipTone).toBe("neutral");
    expect(info.chipEffectiveStatus).toBe("unchecked");
  });

  it("automatic_probe_available:false demotes a reported ready the same way (Gemini Vertex / asr.gladia shape)", () => {
    const info = railEngineInfoForProvider(
      descriptor("Gemini Live"),
      readiness({ status: "ready", automatic_probe_available: false }),
      true,
    );
    expect(info.chipTone).toBe("neutral");
    expect(info.chipEffectiveStatus).toBe("unchecked");
  });

  it("a missing-credentials status still renders (as warning), unaffected by the neutral-only law", () => {
    const info = railEngineInfoForProvider(
      descriptor("AWS Bedrock"),
      readiness({ status: "missing_credentials" }),
      true,
    );
    expect(info.chipRender).toBe(true);
    expect(info.chipTone).toBe("warning");
    expect(info.chipEffectiveStatus).toBe("missing_credentials");
  });

  it("a null readiness (never probed) renders neutral/unchecked, never a fabricated warning", () => {
    const info = railEngineInfoForProvider(
      descriptor("OpenRouter"),
      null,
      true,
    );
    expect(info.chipTone).toBe("neutral");
    expect(info.chipEffectiveStatus).toBe("unchecked");
  });
});
