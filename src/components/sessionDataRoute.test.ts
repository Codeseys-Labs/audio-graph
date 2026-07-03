import { describe, expect, it } from "vitest";
import type { DataMovementEvent } from "../types";
import {
  buildSessionDataRouteReport,
  isContentEgress,
  isEgressBoundary,
} from "./sessionDataRoute";

/** Build a minimal, schema-valid movement event with sensible defaults. */
function event(overrides: Partial<DataMovementEvent> = {}): DataMovementEvent {
  return {
    event_id:
      overrides.event_id ?? `evt-${Math.random().toString(36).slice(2)}`,
    schema_version: 1,
    session_id: "session-1",
    created_at_ms: 1_000,
    actor: "system",
    event_type: "artifact_written",
    destination: { boundary: "local" },
    policy: {
      privacy_mode: "local_only",
      user_visible: true,
      retention_class: "session_artifact",
    },
    result: { status: "succeeded" },
    ...overrides,
  };
}

describe("isEgressBoundary", () => {
  it("treats only local as non-egress", () => {
    expect(isEgressBoundary("local")).toBe(false);
    expect(isEgressBoundary("provider")).toBe(true);
    expect(isEgressBoundary("org")).toBe(true);
    expect(isEgressBoundary("export")).toBe(true);
  });
});

describe("isContentEgress", () => {
  it("is false for a local event", () => {
    expect(
      isContentEgress(
        event({ destination: { boundary: "local" }, data_classes: ["notes"] }),
      ),
    ).toBe(false);
  });

  it("is false for a provider lifecycle event carrying no data classes", () => {
    expect(
      isContentEgress(
        event({
          event_type: "provider_readiness_checked",
          destination: { boundary: "provider", provider_id: "llm.openrouter" },
        }),
      ),
    ).toBe(false);
  });

  it("is false for a policy-blocked provider call (nothing actually left)", () => {
    expect(
      isContentEgress(
        event({
          event_type: "provider_call_started",
          destination: { boundary: "provider", provider_id: "llm.openrouter" },
          data_classes: ["prompts"],
          result: { status: "blocked" },
        }),
      ),
    ).toBe(false);
  });

  it("is true for a provider call carrying content", () => {
    expect(
      isContentEgress(
        event({
          event_type: "provider_call_succeeded",
          destination: { boundary: "provider", provider_id: "llm.openrouter" },
          data_classes: ["transcript_text"],
        }),
      ),
    ).toBe(true);
  });
});

describe("buildSessionDataRouteReport", () => {
  it("reports no content left device for a local-only session", () => {
    const report = buildSessionDataRouteReport([
      event({
        event_type: "capture_started",
        source: { kind: "device", source_label: "Built-in Mic" },
      }),
      event({
        event_type: "artifact_written",
        data_classes: ["transcript_text"],
        destination: { boundary: "local" },
      }),
    ]);

    expect(report.contentLeftDevice).toBe(false);
    expect(report.egressEvents).toHaveLength(0);
    expect(report.localEvents).toHaveLength(2);
    expect(report.providerTransfers).toHaveLength(0);
    expect(report.captureSources).toContain("device: Built-in Mic");
    expect(report.privacyModes).toEqual(["local_only"]);
    expect(report.artifacts.written).toBe(1);
  });

  it("summarizes a cloud transfer with provider/model/data-class and no secret", () => {
    const report = buildSessionDataRouteReport([
      event({
        event_type: "provider_call_succeeded",
        policy: {
          privacy_mode: "byok_cloud",
          user_visible: true,
          retention_class: "transient",
        },
        destination: {
          boundary: "provider",
          provider_id: "llm.openrouter",
          endpoint_class: "chat_completions",
        },
        model: {
          provider_id: "llm.openrouter",
          model_id: "openai/gpt-4o-mini",
        },
        data_classes: ["prompts", "transcript_text"],
      }),
      // A second call to the same provider/model coalesces into one transfer.
      event({
        event_type: "provider_call_succeeded",
        destination: {
          boundary: "provider",
          provider_id: "llm.openrouter",
          endpoint_class: "chat_completions",
        },
        model: {
          provider_id: "llm.openrouter",
          model_id: "openai/gpt-4o-mini",
        },
        data_classes: ["tool_calls"],
      }),
    ]);

    expect(report.contentLeftDevice).toBe(true);
    expect(report.providerTransfers).toHaveLength(1);
    const transfer = report.providerTransfers[0];
    expect(transfer.providerId).toBe("llm.openrouter");
    expect(transfer.modelId).toBe("openai/gpt-4o-mini");
    expect(transfer.endpointClass).toBe("chat_completions");
    expect(transfer.dataClasses.sort()).toEqual(
      ["prompts", "tool_calls", "transcript_text"].sort(),
    );
    expect(report.egressDataClasses.sort()).toEqual(
      ["prompts", "tool_calls", "transcript_text"].sort(),
    );

    // The report carries no secret material — only redaction-safe fields.
    const serialized = JSON.stringify(report).toLowerCase();
    expect(serialized).not.toContain("secret");
    expect(serialized).not.toContain("api_key");
    expect(serialized).not.toContain("bearer");
  });

  it("collects redacted provider errors and a credential readiness summary", () => {
    const report = buildSessionDataRouteReport([
      event({
        event_id: "err-1",
        event_type: "provider_call_failed",
        destination: { boundary: "provider", provider_id: "asr.aws" },
        result: {
          status: "failed",
          error_code: "provider_timeout",
          error_message_redacted: "Request timed out after 30s",
        },
      }),
      event({
        event_type: "provider_readiness_checked",
        destination: { boundary: "local", provider_id: "asr.aws" },
        result: { status: "succeeded" },
      }),
      event({
        event_type: "credential_deleted",
        destination: { boundary: "local", provider_id: "llm.openrouter" },
      }),
    ]);

    expect(report.redactedErrors).toHaveLength(1);
    expect(report.redactedErrors[0].errorCode).toBe("provider_timeout");
    expect(report.redactedErrors[0].message).toBe(
      "Request timed out after 30s",
    );

    const aws = report.credentials.find((c) => c.providerId === "asr.aws");
    expect(aws?.ready).toBe(true);
    const openrouter = report.credentials.find(
      (c) => c.providerId === "llm.openrouter",
    );
    expect(openrouter?.ready).toBe(false);
  });
});
