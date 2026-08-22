import { describe, expect, it } from "vitest";
import { readinessTone as statusTone } from "./badgeTone";
import { readinessAxisTone } from "./readinessTone";

/**
 * The tone LAW, one rule per test (audio-graph-2554 — settings T2). Each test
 * name states the rule from the ratified synthesis (§T2) verbatim so a
 * mutation that removes the corresponding guard fails a clearly-named test.
 * `tone(result)` mirrors exactly what every render site does with the
 * result: the LAW's `forceNeutral` wins, otherwise the site's own
 * status→tone map (here `badgeTone.ts`'s, the one CredentialsPanel and
 * ProviderCapabilityCard actually use) owns the concrete color.
 */
function tone(result: ReturnType<typeof readinessAxisTone>) {
  return result.forceNeutral ? "neutral" : statusTone(result.effectiveStatus);
}

describe("readinessAxisTone — the tone law", () => {
  it("ADR-0030 line 76: a presence-only fixture (no observed status) never renders success or the word Ready", () => {
    // Nothing has been probed yet — only a saved credential (if any) is
    // known. Mutating the helper to stop forcing neutral here on the
    // strength of credential presence alone is exactly the bug ADR-0030
    // forbids.
    const result = readinessAxisTone({ status: undefined, active: true });

    expect(tone(result)).toBe("neutral");
    expect(tone(result)).not.toBe("success");
    expect(result.effectiveStatus).not.toBe("ready");
    expect(result.effectiveStatus).toBe("unchecked");
  });

  it("a fresh, active, ready probe is the ONLY input that earns success + the word Ready", () => {
    const result = readinessAxisTone({
      status: "ready",
      stale: false,
      active: true,
    });

    expect(tone(result)).toBe("success");
    expect(result.render).toBe(true);
    expect(result.effectiveStatus).toBe("ready");
  });

  it("stale demotes tone (never merely an appended sentence) — a cached ready never renders success", () => {
    const result = readinessAxisTone({
      status: "ready",
      stale: true,
      active: true,
    });

    expect(tone(result)).not.toBe("success");
    expect(tone(result)).toBe("neutral");
    expect(result.effectiveStatus).not.toBe("ready");
    expect(result.effectiveStatus).toBe("unchecked");
    expect(result.render).toBe(true);
  });

  it("automatic_probe_available:false renders neutral, never warning (asr.gladia / Gemini Vertex shape)", () => {
    // Mirrors the real fixtures in ProviderReadinessPanel.test.tsx:535,549 —
    // both report status "unchecked" because this app cannot auto-probe them.
    const result = readinessAxisTone({
      status: "unchecked",
      automaticProbeAvailable: false,
      active: false,
    });

    expect(tone(result)).toBe("neutral");
    expect(tone(result)).not.toBe("warning");
    expect(result.render).toBe(true);
  });

  it("automatic_probe_available:false does NOT demote a real missing_credentials/error status — this is the production-normal shape, not a hypothetical", () => {
    // Retargeted: this test previously asserted the OPPOSITE (forceNeutral)
    // for this exact fixture, calling it "a hypothetical" — it is not. The
    // backend sets `automatic_probe_available: false` UNCONDITIONALLY
    // whenever required credentials are missing
    // (`automatic_probe_available_from_decision`, commands.rs, short-
    // circuits on `missing.is_empty()`), so EVERY `missing_credentials`
    // readiness entry in production also carries
    // `automatic_probe_available: false`. The old assertion meant every
    // "you haven't saved this key yet" chip in the app — the single most
    // actionable state in the settings tree — silently lost its amber
    // `warning` tone to neutral. `automatic_probe_available` only demotes a
    // reported "ready" (mirroring `providerRecoveryAction`, which likewise
    // only consults it inside `case "unchecked":`); a real structural
    // blocker keeps its own tone regardless.
    const result = readinessAxisTone({
      status: "missing_credentials",
      automaticProbeAvailable: false,
      active: true,
    });

    expect(result.forceNeutral).toBe(false);
    expect(tone(result)).toBe("warning");
    expect(result.effectiveStatus).toBe("missing_credentials");
  });

  it("automatic_probe_available:false demotes a reported 'ready' the same way stale does — tone AND the word 'Ready' both collapse", () => {
    // The one shape where automatic_probe_available legitimately overrides:
    // a "ready" this app can no longer automatically re-verify is exactly as
    // untrustworthy as a stale one, and must demote identically (not just
    // the tone, which would leave the word "Ready" rendering at a neutral
    // color — see effectiveStatus below).
    const result = readinessAxisTone({
      status: "ready",
      automaticProbeAvailable: false,
      active: true,
    });

    expect(result.forceNeutral).toBe(true);
    expect(tone(result)).toBe("neutral");
    expect(result.effectiveStatus).toBe("unchecked");
    expect(result.effectiveStatus).not.toBe("ready");
  });

  it("a non-active provider gets NO axis-3 chip at all, even if its last known status was ready", () => {
    const result = readinessAxisTone({
      status: "ready",
      stale: false,
      active: false,
    });

    expect(result.render).toBe(false);
    expect(tone(result)).not.toBe("success");
  });

  it("a saved-key-but-error fixture never renders success anywhere, active or not", () => {
    expect(tone(readinessAxisTone({ status: "error", active: true }))).toBe(
      "danger",
    );
    expect(tone(readinessAxisTone({ status: "error", active: false }))).toBe(
      "danger",
    );
  });

  it("missing_credentials still renders (a real structural blocker), independent of active use", () => {
    const result = readinessAxisTone({
      status: "missing_credentials",
      active: false,
    });

    expect(result.render).toBe(true);
    expect(tone(result)).toBe("warning");
  });

  it("is generic over a caller's own status enum (e.g. the mode-card aggregate's 'blocked', absent from ProviderReadinessStatus)", () => {
    // ProductModeSummaryCards feeds ProviderSetupReadinessStatus (which adds
    // "blocked") through the same law; the caller's own status→tone map
    // still owns "blocked", the law only decides render/forceNeutral.
    const result = readinessAxisTone<
      "ready" | "missing_credentials" | "blocked" | "error" | "unchecked"
    >({ status: "blocked", active: true });

    expect(result.effectiveStatus).toBe("blocked");
    expect(result.forceNeutral).toBe(false);
    expect(result.render).toBe(true);
  });
});
