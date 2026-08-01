import { describe, expect, expectTypeOf, it } from "vitest";
import {
  DURABLE_CLOUD_ASR_CREDENTIAL_KEYS,
  DURABLE_CLOUD_LLM_CREDENTIAL_KEYS,
} from "../credentialKeys";
import type {
  CredentialIdempotencyToken,
  CredentialOperationId,
  CredentialRevision,
} from "./credentialContract";
import {
  ALLOWED_CREDENTIAL_KEYS,
  CREDENTIAL_CONTRACT,
  PORTABLE_ENCODED_RECORD_MAX_BYTES,
} from "./credentialContract";

const field = (legacyKey: string) => {
  const definition = CREDENTIAL_CONTRACT.fields.find(
    (candidate) => candidate.legacy_key === legacyKey,
  );
  if (!definition) {
    throw new Error(`missing credential field ${legacyKey}`);
  }
  return definition;
};

describe("generated credential contract", () => {
  it("projects all 22 v1 keys exactly once with explicit dispositions", () => {
    expect(ALLOWED_CREDENTIAL_KEYS).toHaveLength(22);
    expect(new Set(ALLOWED_CREDENTIAL_KEYS).size).toBe(22);
    expect(
      CREDENTIAL_CONTRACT.fields.every((definition) =>
        ["migrate", "config", "deprecate", "remove"].includes(
          definition.legacy_disposition,
        ),
      ),
    ).toBe(true);
  });

  it("keeps AWS static credentials atomic and ordinary config outside presence", () => {
    const access = field("aws_access_key");
    const secret = field("aws_secret_key");
    const session = field("aws_session_token");
    expect(access.set_id).toBe("aws");
    expect(secret.set_id).toBe("aws");
    expect(session.set_id).toBe("aws");
    expect(access.requirement).toEqual({
      kind: "required_together",
      group_id: "aws.static_pair",
    });
    expect(secret.requirement).toEqual(access.requirement);
    expect(session.requirement).toEqual({ kind: "optional" });
    expect(session.contributes_to_credential_presence).toBe(false);
    expect(
      CREDENTIAL_CONTRACT.sets.find((set) => set.id === "aws")?.configured_when,
    ).toEqual({ kind: "required_together", group_id: "aws.static_pair" });

    for (const key of ["aws_profile", "aws_region"] as const) {
      expect(field(key)).toMatchObject({
        class: "ordinary_config",
        legacy_disposition: "config",
        contributes_to_credential_presence: false,
      });
    }
  });

  it("models Gemini key and service-account locator as safe alternatives", () => {
    expect(field("gemini_api_key")).toMatchObject({
      set_id: "gemini",
      class: "secret",
      requirement: {
        kind: "alternative",
        group_id: "gemini.authentication",
      },
      contributes_to_credential_presence: true,
    });
    expect(field("google_service_account_path")).toMatchObject({
      set_id: "gemini",
      class: "private_locator",
      legacy_disposition: "config",
      requirement: {
        kind: "alternative",
        group_id: "gemini.authentication",
      },
      contributes_to_credential_presence: false,
    });
    expect(
      CREDENTIAL_CONTRACT.sets.find((set) => set.id === "gemini")
        ?.configured_when,
    ).toEqual({
      kind: "any_stored_secret_alternative",
      group_id: "gemini.authentication",
    });
  });

  it("maps durable cloud keys to concrete authorized or disabled relations", () => {
    for (const keys of [
      DURABLE_CLOUD_ASR_CREDENTIAL_KEYS,
      DURABLE_CLOUD_LLM_CREDENTIAL_KEYS,
    ]) {
      for (const key of keys) {
        const definition = field(key);
        const relations = CREDENTIAL_CONTRACT.use_policies.filter(
          (policy) =>
            policy.set_id === definition.set_id &&
            definition.auth_method_ids.some(
              (authMethod) => String(authMethod) === policy.auth_method_id,
            ),
        );
        expect(
          relations.length,
          `${key} should have a concrete relation`,
        ).toBeGreaterThan(0);
        expect(
          relations.every((policy) =>
            ["authorized", "disabled"].includes(policy.decision.status),
          ),
        ).toBe(true);
      }
    }

    const gemini = CREDENTIAL_CONTRACT.use_policies.filter(
      (policy) => policy.set_id === "gemini",
    );
    expect(
      gemini.some(
        (policy) => policy.consumer_id === "realtime_agent.gemini_live",
      ),
    ).toBe(true);
    expect(
      gemini.some((policy) => String(policy.consumer_id) === "llm.api"),
    ).toBe(false);

    const azure = CREDENTIAL_CONTRACT.use_policies.find(
      (policy) => policy.set_id === "azure_speech",
    );
    expect(azure).toMatchObject({
      consumer_id: "asr.azure_speech",
      decision: { status: "disabled", reason: "provider_planned" },
      active_use_action: "stop",
    });
  });

  it("binds Gemini auth modes to disjoint canonical audiences", () => {
    const gemini = CREDENTIAL_CONTRACT.use_policies.filter(
      (policy) => policy.set_id === "gemini",
    );
    const apiKeyAudiences = gemini
      .filter((policy) => policy.auth_method_id === "api_key")
      .map((policy) => policy.decision);
    expect(apiKeyAudiences).toEqual(
      expect.arrayContaining([
        {
          status: "authorized",
          audience: {
            kind: "exact_secure_origin",
            origin: "wss://generativelanguage.googleapis.com",
          },
        },
      ]),
    );

    const vertex = gemini.find(
      (policy) => policy.auth_method_id === "google_service_account_file",
    );
    expect(vertex?.decision).toEqual({
      status: "authorized",
      audience: {
        kind: "backend_derived_vertex_origin",
        scheme: "wss",
        host_suffix: "aiplatform.googleapis.com",
        effective_port: 443,
      },
    });
  });

  it("exports distinct opaque token brands", () => {
    expectTypeOf<CredentialRevision>().not.toEqualTypeOf<string>();
    expectTypeOf<CredentialRevision>().not.toEqualTypeOf<CredentialOperationId>();
    expectTypeOf<CredentialOperationId>().not.toEqualTypeOf<CredentialIdempotencyToken>();
  });

  it("exposes the exact portable ceiling and closed custom-id policy", () => {
    expect(PORTABLE_ENCODED_RECORD_MAX_BYTES).toBe(2560);
    expect(CREDENTIAL_CONTRACT.custom_set_policy).toMatchObject({
      id_prefix: "custom.",
      backend_issued: true,
      immutable_origin_binding: true,
      complete_secret_required_for_new_binding: true,
      allowed_schemes: ["https", "wss"],
    });
  });
});
