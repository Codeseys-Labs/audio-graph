/**
 * Settings T1 (seed audio-graph-2b9a) — pure route-table unit tests.
 *
 * These pin the extraction's behavior-identity: every case here is a
 * literal, git-show-verified transcription of the pre-extraction closures
 * that used to live inline in `useSettingsController.tsx` (compare against
 * `git show 0cb9f98:src/components/settings/useSettingsController.tsx` —
 * the merge-base commit immediately before this seed landed — the `apply`
 * closures there and the `applyAction` data here encode the identical
 * decision). A mutation to any
 * `tab`/`fieldId`/`applyAction` literal below is exactly the "route table
 * entry changed" failure the drift tripwire (`SettingsPage.test.tsx`) and
 * these spot checks are meant to catch.
 */
import { describe, expect, it } from "vitest";
import type { SettingsRoute } from "../../types";
import {
  credentialRouteForKey,
  credentialRouteForProviderCredential,
  credentialRouteForProviderSetupSelection,
  credentialRouteForReadiness,
  fallbackCredentialRouteForKey,
  modelRouteForProviderId,
  providerRouteForProviderId,
  providerRouteForStage,
  ROUTE_INDEX,
  relatedReadinessForCredential,
} from "./settingsRoutes";

describe("credentialRouteForProviderCredential", () => {
  it("routes cloud ASR providers to their credential field with activate + a variant apply", () => {
    expect(credentialRouteForProviderCredential("asr.deepgram", null)).toEqual({
      tab: "stt",
      fieldId: "deepgram-api-key",
      activate: true,
      applyAction: { kind: "asr_variant", variant: "deepgram" },
    });
    expect(
      credentialRouteForProviderCredential("asr.assemblyai", null),
    ).toEqual({
      tab: "stt",
      fieldId: "assemblyai-api-key",
      activate: true,
      applyAction: { kind: "asr_variant", variant: "assemblyai" },
    });
  });

  it("aws_transcribe's credential route also flips the AWS auth-mode toggle", () => {
    expect(
      credentialRouteForProviderCredential("asr.aws_transcribe", null),
    ).toEqual({
      tab: "stt",
      fieldId: "aws-asr-access-key",
      activate: true,
      applyAction: { kind: "asr_aws_transcribe_credential" },
    });
  });

  it("llm.aws_bedrock's credential route also flips its own AWS auth-mode toggle", () => {
    expect(
      credentialRouteForProviderCredential("llm.aws_bedrock", null),
    ).toEqual({
      tab: "llm",
      fieldId: "llm-bedrock-access-key",
      activate: true,
      applyAction: { kind: "llm_aws_bedrock_credential" },
    });
  });

  it("gates the native OpenAI realtime-agent route on the openai_api_key credential, never mutating asrType", () => {
    expect(
      credentialRouteForProviderCredential(
        "realtime_agent.openai_realtime",
        "openai_api_key",
      ),
    ).toEqual({
      tab: "gemini",
      fieldId: "settings-provider-capability-realtime_agent.openai_realtime",
      activate: true,
    });
    // Any other credential key (including null) must not match — this is
    // the split-brain guard: pointing here must never force-select the
    // pipeline STT provider.
    expect(
      credentialRouteForProviderCredential(
        "realtime_agent.openai_realtime",
        null,
      ),
    ).toBeNull();
  });

  it("gates the Gemini Live api_key route on the gemini_api_key credential", () => {
    expect(
      credentialRouteForProviderCredential(
        "realtime_agent.gemini_live",
        "gemini_api_key",
      ),
    ).toEqual({
      tab: "gemini",
      fieldId: "gemini-api-key",
      activate: true,
      applyAction: { kind: "gemini_api_key" },
    });
    expect(
      credentialRouteForProviderCredential("realtime_agent.gemini_live", null),
    ).toBeNull();
  });

  it("returns null for a provider id with no credential route (e.g. local-only providers)", () => {
    expect(
      credentialRouteForProviderCredential("asr.local_whisper", null),
    ).toBeNull();
    expect(
      credentialRouteForProviderCredential("llm.local_llama", null),
    ).toBeNull();
  });
});

describe("credentialRouteForReadiness", () => {
  it("delegates to credentialRouteForProviderCredential using the entry's own provider id + first credential key", () => {
    const route = credentialRouteForReadiness({
      provider_id: "asr.deepgram",
      status: "missing_credentials",
      message: "",
      stale: false,
      credential_epoch: 1,
      credentials: [{ key: "deepgram_api_key", present: false }],
    });
    expect(route).toEqual({
      tab: "stt",
      fieldId: "deepgram-api-key",
      activate: true,
      applyAction: { kind: "asr_variant", variant: "deepgram" },
    });
  });
});

describe("providerRouteForProviderId", () => {
  it("routes a plain provider inspect action without a credential-mode side effect (aws_transcribe: single dispatch, not the credential route's double dispatch)", () => {
    expect(providerRouteForProviderId("asr.aws_transcribe")).toEqual({
      tab: "stt",
      fieldId: "aws-asr-region",
      applyAction: { kind: "asr_variant", variant: "aws_transcribe" },
    });
  });

  it("routes tts.none/tts.deepgram_aura to the same select field with the matching variant apply", () => {
    expect(providerRouteForProviderId("tts.none")).toEqual({
      tab: "tts",
      fieldId: "tts-provider-select",
      applyAction: { kind: "tts_variant", variant: "none" },
    });
    expect(providerRouteForProviderId("tts.deepgram_aura")).toEqual({
      tab: "tts",
      fieldId: "tts-provider-select",
      applyAction: { kind: "tts_variant", variant: "deepgram_aura" },
    });
  });

  it("routes realtime_agent.gemini_live with no apply at all (navigate only)", () => {
    expect(providerRouteForProviderId("realtime_agent.gemini_live")).toEqual({
      tab: "gemini",
      fieldId: "gemini-model",
    });
  });

  it("returns null for an unknown provider id", () => {
    expect(providerRouteForProviderId("asr.nonexistent")).toBeNull();
  });
});

describe("providerRouteForStage", () => {
  it("is providerRouteForProviderId keyed by stage + settings-variant string (ExpressSetup's Advanced handoff)", () => {
    expect(providerRouteForStage("asr", "deepgram")).toEqual(
      providerRouteForProviderId("asr.deepgram"),
    );
    expect(providerRouteForStage("llm", "api")).toEqual(
      providerRouteForProviderId("llm.api"),
    );
  });

  it("returns null for a settings variant with no provider route (e.g. a stage/variant that doesn't exist)", () => {
    expect(providerRouteForStage("asr", "nonexistent")).toBeNull();
  });
});

describe("modelRouteForProviderId", () => {
  it("delegates local_whisper's model route to its plain provider route (same single field for both)", () => {
    expect(modelRouteForProviderId("asr.local_whisper")).toEqual(
      providerRouteForProviderId("asr.local_whisper"),
    );
  });

  it("routes llm.local_llama's model route to the Credentials tab's Models section, with no apply (T4a moved this from General)", () => {
    expect(modelRouteForProviderId("llm.local_llama")).toEqual({
      tab: "credentials",
      fieldId: "settings-models-section",
    });
  });

  it("returns null for a provider id with no model route (e.g. tts.none)", () => {
    expect(modelRouteForProviderId("tts.none")).toBeNull();
  });
});

describe("credentialRouteForProviderSetupSelection", () => {
  it("routes the aura TTS special case to its own credential field, not the generic tts credential route", () => {
    expect(
      credentialRouteForProviderSetupSelection("tts.deepgram_aura", null),
    ).toEqual({
      tab: "tts",
      fieldId: "tts-deepgram-api-key",
      applyAction: { kind: "tts_variant", variant: "deepgram_aura" },
    });
  });

  it("routes the Gemini Vertex service-account special case, flipping auth mode to vertex_ai", () => {
    expect(
      credentialRouteForProviderSetupSelection(
        "realtime_agent.gemini_live",
        "google_service_account_path",
      ),
    ).toEqual({
      tab: "gemini",
      fieldId: "gemini-service-account-path",
      applyAction: { kind: "gemini_vertex_ai" },
    });
  });

  it("delegates every other provider id to credentialRouteForProviderCredential", () => {
    expect(
      credentialRouteForProviderSetupSelection("asr.deepgram", null),
    ).toEqual(credentialRouteForProviderCredential("asr.deepgram", null));
  });
});

describe("fallbackCredentialRouteForKey", () => {
  it("routes every cloud credential key to its provider's credential field", () => {
    expect(fallbackCredentialRouteForKey("openrouter_api_key")).toEqual({
      tab: "llm",
      fieldId: "llm-openrouter-api-key",
      activate: true,
      applyAction: { kind: "llm_variant", variant: "openrouter" },
    });
    expect(fallbackCredentialRouteForKey("aws_secret_key")).toEqual({
      tab: "stt",
      fieldId: "aws-asr-access-key",
      activate: true,
      applyAction: { kind: "asr_aws_transcribe_credential" },
    });
  });

  it("returns null for openai_api_key (ambiguous across providers, resolved elsewhere) and unknown keys", () => {
    expect(fallbackCredentialRouteForKey("openai_api_key")).toBeNull();
    expect(fallbackCredentialRouteForKey("not_a_real_key")).toBeNull();
  });
});

describe("credentialRouteForKey", () => {
  const baseCtx = {
    activeReadinessProviderIds: [] as string[],
    providerReadinessEntries: [],
    asrType: "local_whisper" as const,
    llmType: "api" as const,
    asrEndpointCredentialKey: null,
    llmEndpointCredentialKey: null,
  };

  it("prefers the active provider that owns the endpoint-derived credential key over the static fallback", () => {
    const route = credentialRouteForKey("cerebras_api_key", {
      ...baseCtx,
      activeReadinessProviderIds: ["llm.api"],
      llmType: "api",
      llmEndpointCredentialKey: "cerebras_api_key",
    });
    expect(route).toEqual({
      tab: "llm",
      fieldId: "llm-custom-api-key",
      activate: true,
      applyAction: { kind: "llm_variant", variant: "api" },
    });
  });

  it("falls back to the static per-key route when no active provider claims the key", () => {
    const route = credentialRouteForKey("sambanova_api_key", baseCtx);
    expect(route).toEqual(fallbackCredentialRouteForKey("sambanova_api_key"));
  });

  // `openai_api_key` is the one credential shared across four provider ids
  // (llm.api, asr.api, asr.openai_realtime, realtime_agent.openai_realtime),
  // so `credentialRouteForKey` special-cases it with its own ladder
  // (`activeOpenAiCredentialRoute` then `readinessOpenAiCredentialRoute`'s
  // `["llm.api", "asr.api", "asr.openai_realtime"]` priority list, falling
  // through to any other readiness entry that mentions the key). This was
  // previously untested — verified line-by-line against
  // `git show 0cb9f98:src/components/settings/useSettingsController.tsx`
  // to be a faithful (identical conditions, order, and fallthrough)
  // extraction.
  describe("openai_api_key resolution ladder", () => {
    it("prefers the active llm.api endpoint over an also-active asr.api endpoint", () => {
      const route = credentialRouteForKey("openai_api_key", {
        ...baseCtx,
        llmType: "api",
        llmEndpointCredentialKey: "openai_api_key",
        asrType: "api",
        asrEndpointCredentialKey: "openai_api_key",
      });
      expect(route).toEqual({
        tab: "llm",
        fieldId: "llm-custom-api-key",
        activate: true,
        applyAction: { kind: "llm_variant", variant: "api" },
      });
    });

    it("falls to the active asr.api endpoint when llm doesn't claim the key", () => {
      const route = credentialRouteForKey("openai_api_key", {
        ...baseCtx,
        llmType: "local_llama",
        asrType: "api",
        asrEndpointCredentialKey: "openai_api_key",
      });
      expect(route).toEqual({
        tab: "stt",
        fieldId: "asr-api-key",
        activate: true,
        applyAction: { kind: "asr_variant", variant: "api" },
      });
    });

    it("falls to the active asr.openai_realtime provider when neither llm nor asr.api claim the key", () => {
      const route = credentialRouteForKey("openai_api_key", {
        ...baseCtx,
        llmType: "local_llama",
        asrType: "openai_realtime",
      });
      expect(route).toEqual({
        tab: "stt",
        fieldId: "openai-realtime-api-key",
        activate: true,
        applyAction: { kind: "asr_variant", variant: "openai_realtime" },
      });
    });

    function readinessEntry(providerId: string) {
      return {
        provider_id: providerId,
        status: "missing_credentials" as const,
        message: "",
        stale: false,
        credential_epoch: 1,
        credentials: [{ key: "openai_api_key", present: false }],
      };
    }

    it("prefers the llm.api readiness entry over asr.api/asr.openai_realtime when no provider is actively configured", () => {
      const route = credentialRouteForKey("openai_api_key", {
        ...baseCtx,
        llmType: "local_llama",
        asrType: "local_whisper",
        providerReadinessEntries: [
          readinessEntry("asr.openai_realtime"),
          readinessEntry("asr.api"),
          readinessEntry("llm.api"),
        ],
      });
      expect(route).toEqual({
        tab: "llm",
        fieldId: "llm-custom-api-key",
        activate: true,
        applyAction: { kind: "llm_variant", variant: "api" },
      });
    });

    it("falls to the asr.api readiness entry when no llm.api readiness entry exists", () => {
      const route = credentialRouteForKey("openai_api_key", {
        ...baseCtx,
        llmType: "local_llama",
        asrType: "local_whisper",
        providerReadinessEntries: [
          readinessEntry("asr.openai_realtime"),
          readinessEntry("asr.api"),
        ],
      });
      expect(route).toEqual({
        tab: "stt",
        fieldId: "asr-api-key",
        activate: true,
        applyAction: { kind: "asr_variant", variant: "api" },
      });
    });

    it("falls to the asr.openai_realtime readiness entry when neither llm.api nor asr.api readiness entries exist", () => {
      const route = credentialRouteForKey("openai_api_key", {
        ...baseCtx,
        llmType: "local_llama",
        asrType: "local_whisper",
        providerReadinessEntries: [readinessEntry("asr.openai_realtime")],
      });
      expect(route).toEqual({
        tab: "stt",
        fieldId: "openai-realtime-api-key",
        activate: true,
        applyAction: { kind: "asr_variant", variant: "openai_realtime" },
      });
    });

    it("falls to any other readiness entry mentioning the key when none of the three priority ids match", () => {
      const route = credentialRouteForKey("openai_api_key", {
        ...baseCtx,
        llmType: "local_llama",
        asrType: "local_whisper",
        providerReadinessEntries: [
          readinessEntry("realtime_agent.openai_realtime"),
        ],
      });
      expect(route).toEqual({
        tab: "gemini",
        fieldId: "settings-provider-capability-realtime_agent.openai_realtime",
        activate: true,
      });
    });

    it("returns null when nothing active or readiness-listed claims the key", () => {
      const route = credentialRouteForKey("openai_api_key", {
        ...baseCtx,
        llmType: "local_llama",
        asrType: "local_whisper",
      });
      expect(route).toBeNull();
    });
  });
});

describe("relatedReadinessForCredential", () => {
  it("finds every readiness entry that lists the credential key", () => {
    const entries = [
      {
        provider_id: "asr.deepgram",
        status: "ready" as const,
        message: "",
        stale: false,
        credential_epoch: 1,
        credentials: [{ key: "deepgram_api_key", present: true }],
      },
      {
        provider_id: "llm.openrouter",
        status: "missing_credentials" as const,
        message: "",
        stale: false,
        credential_epoch: 1,
        credentials: [{ key: "openrouter_api_key", present: false }],
      },
    ];
    expect(
      relatedReadinessForCredential("deepgram_api_key", entries).map(
        (e) => e.provider_id,
      ),
    ).toEqual(["asr.deepgram"]);
    expect(relatedReadinessForCredential("no_such_key", entries)).toEqual([]);
  });
});

describe("ROUTE_INDEX", () => {
  it("is non-empty, has no duplicate tab:fieldId pairs, and every entry has a real tab + fieldId", () => {
    expect(ROUTE_INDEX.length).toBeGreaterThan(20);
    const keys = ROUTE_INDEX.map((r) => `${r.tab}:${r.fieldId}`);
    expect(new Set(keys).size).toBe(keys.length);
    for (const { tab, fieldId } of ROUTE_INDEX) {
      expect(tab.length).toBeGreaterThan(0);
      expect(fieldId.length).toBeGreaterThan(0);
    }
  });
});

describe("external SettingsRoute has no mutation hook (type-level pin)", () => {
  it("compiles a plain navigate-only route, and rejects one carrying apply/applyAction", () => {
    // The external, cross-cutting `SettingsRoute` (store/openSettings) must
    // never widen to carry a mutation hook — see its doc comment in
    // `types/index.ts`. This is the exact shape every external caller
    // (PreflightCard, ExpressSetup, the keyboard shortcut, ...) is allowed
    // to build.
    const route: SettingsRoute = {
      tab: "llm",
      fieldId: "llm-custom-api-key",
      activate: true,
    };
    expect(route.tab).toBe("llm");

    // @ts-expect-error — SettingsRoute has no `apply` member; only the
    // controller-internal RouteEntry/ApplyAction pair may carry a mutation.
    const withApply: SettingsRoute = { ...route, apply: () => {} };
    void withApply;
  });
});
