import { fireEvent, render, screen, within } from "@testing-library/react";
import i18n from "i18next";
import { describe, expect, it } from "vitest";
import { GENERATED_PROVIDER_REGISTRY } from "../generated/providerRegistry";
import { RUNTIME_DIAGNOSTIC_CONTRACT } from "../generated/runtimeDiagnostic";
import {
  credentialStoreFailure,
  FAILURE_CASES,
  failedReadiness,
  READINESS_NOTE_CASES,
  runtimeFailure,
} from "../test/readinessFixtures";
import type { ProviderReadiness, ProviderRuntimeReadiness } from "../types";
import ProviderReadinessPanel, {
  providerCatalogSummary,
  providerReadinessMessage,
  providerRecoveryAction,
  providerRuntimeMessage,
} from "./ProviderReadinessPanel";
import "../i18n";

const t = i18n.getFixedT("en");

function readiness(
  overrides: Partial<ProviderReadiness> = {},
): ProviderReadiness {
  return {
    provider_id: "asr.deepgram",
    status: "unchecked",
    note: { kind: "ready_to_check" },
    automatic_probe_available: true,
    checked_at: null,
    stale: false,
    credential_epoch: 0,
    credentials: [{ key: "deepgram_api_key", present: true }],
    ...overrides,
  };
}

describe("ProviderReadinessPanel", () => {
  it("keeps healthy final-only readiness separate from recovery-neutral fidelity limits", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          provider_id: "asr.api",
          status: "ready",
          note: { kind: "verified" },
          credentials: [],
          effective_stt_fidelity: {
            revision_semantics: "final_only",
            timing: "app_estimated",
            confidence: "unavailable",
            turn: "unavailable",
            speaker: "unavailable",
            channel: "unavailable",
            turn_detection: {
              speech_start: false,
              speech_final: false,
              endpointing_configured: false,
              utterance_end: false,
              end_of_turn: false,
              eager_end_of_turn: false,
              turn_resume: false,
            },
            degradations: [
              "final_only_revisions",
              "app_estimated_timing",
              "confidence_unavailable",
              "turn_unavailable",
              "speaker_unavailable",
              "channel_unavailable",
            ],
          },
        })}
        loading={false}
        t={t}
      />,
    );

    const operationalStatus = screen.getByRole("status");
    expect(operationalStatus).toHaveTextContent(/Ready/i);
    expect(operationalStatus).toHaveTextContent(/Provider check passed/i);

    const fidelity = screen.getByRole("region", {
      name: /transcription fidelity/i,
    });
    expect(fidelity).toHaveTextContent(/Reduced transcript detail/i);
    expect(fidelity).toHaveTextContent(/No action is required/i);
    expect(fidelity).toHaveTextContent(/Final results only/i);
    expect(fidelity).toHaveTextContent(/Estimated by AudioGraph/i);
    expect(fidelity).toHaveTextContent(/Confidence\s*Not available/i);
    expect(fidelity).toHaveTextContent(/Turns\s*Not available/i);
    expect(fidelity).toHaveTextContent(/Speaker labels\s*Not available/i);
    expect(fidelity).toHaveTextContent(/Channels\s*Not available/i);
    expect(fidelity).not.toHaveTextContent(/^Next\b/i);
  });

  it("uses typed effective fields, not the Deepgram provider id, for enabled and disabled speaker labels", () => {
    const enabled = readiness({
      provider_id: "asr.deepgram",
      status: "ready",
      effective_stt_fidelity: {
        revision_semantics: "partial_and_final",
        timing: "provider_exact",
        confidence: "provider",
        turn: "provider",
        speaker: "provider",
        channel: "unavailable",
        turn_detection: {
          speech_start: true,
          speech_final: true,
          endpointing_configured: true,
          utterance_end: true,
          end_of_turn: false,
          eager_end_of_turn: false,
          turn_resume: false,
        },
        degradations: ["channel_unavailable"],
      },
    });
    const enabledFidelity = enabled.effective_stt_fidelity;
    if (!enabledFidelity) throw new Error("Deepgram fidelity fixture missing");
    const view = render(
      <ProviderReadinessPanel entry={enabled} loading={false} t={t} />,
    );

    let fidelity = screen.getByRole("region", {
      name: /transcription fidelity/i,
    });
    expect(fidelity).toHaveTextContent(/Speaker labels\s*Provider-reported/i);
    expect(fidelity).toHaveTextContent(/Speech-start events\s*Enabled/i);
    expect(fidelity).toHaveTextContent(/Speech-final events\s*Enabled/i);
    expect(fidelity).toHaveTextContent(/Endpointing\s*Enabled/i);
    expect(fidelity).not.toHaveTextContent(/Endpointing\s*Provider default/i);
    expect(fidelity).toHaveTextContent(/Utterance-end events\s*Enabled/i);
    expect(fidelity).toHaveTextContent(/End-of-turn events\s*Disabled/i);
    expect(fidelity).toHaveTextContent(/Eager end-of-turn\s*Disabled/i);
    expect(fidelity).toHaveTextContent(/Turn-resume events\s*Disabled/i);
    expect(fidelity).not.toHaveTextContent(/Disabled in settings/i);

    view.rerender(
      <ProviderReadinessPanel
        entry={{
          ...enabled,
          effective_stt_fidelity: {
            ...enabledFidelity,
            speaker: "unavailable",
            turn_detection: {
              speech_start: false,
              speech_final: false,
              endpointing_configured: false,
              utterance_end: false,
              end_of_turn: true,
              eager_end_of_turn: true,
              turn_resume: true,
            },
            degradations: [
              "speaker_disabled_by_configuration",
              "channel_unavailable",
            ],
          },
        }}
        loading={false}
        t={t}
      />,
    );

    fidelity = screen.getByRole("region", {
      name: /transcription fidelity/i,
    });
    expect(fidelity).toHaveTextContent(
      /Speaker labels\s*Disabled in settings/i,
    );
    expect(fidelity).toHaveTextContent(
      /Speaker labels are disabled by the selected settings/i,
    );
    expect(fidelity).toHaveTextContent(/Speech-start events\s*Disabled/i);
    expect(fidelity).toHaveTextContent(/Speech-final events\s*Disabled/i);
    expect(fidelity).toHaveTextContent(/Endpointing\s*Provider default/i);
    expect(fidelity).not.toHaveTextContent(/Endpointing\s*(Enabled|Disabled)/i);
    expect(fidelity).toHaveTextContent(/Utterance-end events\s*Disabled/i);
    expect(fidelity).toHaveTextContent(/End-of-turn events\s*Enabled/i);
    expect(fidelity).toHaveTextContent(/Eager end-of-turn\s*Enabled/i);
    expect(fidelity).toHaveTextContent(/Turn-resume events\s*Enabled/i);
    expect(fidelity).not.toHaveTextContent(
      /Speaker labels\s*Provider-reported/i,
    );
  });

  it("omits absent fidelity and safely localizes unknown future fields without echoing diagnostics", () => {
    const view = render(
      <ProviderReadinessPanel
        entry={readiness({ status: "ready" })}
        loading={false}
        t={t}
      />,
    );

    expect(
      screen.queryByRole("region", { name: /transcription fidelity/i }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(/Ready/i);

    view.rerender(
      <ProviderReadinessPanel
        entry={readiness({
          status: "ready",
          effective_stt_fidelity: {
            revision_semantics: "future_revision_mode",
            degradations: [
              "future_backend_detail: transcript=private sk-secret",
            ],
          } as unknown as NonNullable<
            ProviderReadiness["effective_stt_fidelity"]
          >,
        })}
        loading={false}
        t={t}
      />,
    );

    const fidelity = screen.getByRole("region", {
      name: /transcription fidelity/i,
    });
    expect(fidelity).toHaveTextContent(/Fidelity details incomplete/i);
    expect(fidelity).not.toHaveTextContent(/Full transcript detail/i);
    expect(fidelity).toHaveTextContent(/Not reported/i);
    expect(fidelity).toHaveTextContent(/Some transcription detail is reduced/i);
    expect(fidelity).not.toHaveTextContent(/future_backend_detail/i);
    expect(fidelity).not.toHaveTextContent(/private|sk-secret/i);
  });

  it("treats missing or malformed degradation arrays as incomplete version-skewed fidelity", () => {
    const completeFields = {
      revision_semantics: "partial_and_final",
      timing: "provider_exact",
      confidence: "provider",
      turn: "provider",
      speaker: "provider",
      channel: "provider",
      turn_detection: {
        speech_start: true,
        speech_final: true,
        endpointing_configured: true,
        utterance_end: true,
        end_of_turn: true,
        eager_end_of_turn: true,
        turn_resume: true,
      },
    };
    const view = render(
      <ProviderReadinessPanel
        entry={readiness({
          status: "ready",
          effective_stt_fidelity: completeFields as unknown as NonNullable<
            ProviderReadiness["effective_stt_fidelity"]
          >,
        })}
        loading={false}
        t={t}
      />,
    );

    let fidelity = screen.getByRole("region", {
      name: /transcription fidelity/i,
    });
    expect(fidelity).toHaveTextContent(/Fidelity details incomplete/i);
    expect(fidelity).not.toHaveTextContent(/Full transcript detail/i);

    view.rerender(
      <ProviderReadinessPanel
        entry={readiness({
          status: "ready",
          effective_stt_fidelity: {
            ...completeFields,
            degradations: { malformed: true },
          } as unknown as NonNullable<
            ProviderReadiness["effective_stt_fidelity"]
          >,
        })}
        loading={false}
        t={t}
      />,
    );

    fidelity = screen.getByRole("region", {
      name: /transcription fidelity/i,
    });
    expect(fidelity).toHaveTextContent(/Fidelity details incomplete/i);
    expect(fidelity).not.toHaveTextContent(/Full transcript detail/i);
  });

  it("guides users to add missing credentials without rendering secret values", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          status: "missing_credentials",
          note: { kind: "missing_credentials", keys: ["deepgram_api_key"] },
          credentials: [{ key: "deepgram_api_key", present: false }],
        })}
        loading={false}
        t={t}
      />,
    );

    expect(
      screen.getByText(/add the missing key in this provider section/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/sk-/i)).not.toBeInTheDocument();
  });

  it("does not point at the saved key when the provider rejected the request itself", () => {
    const entry = readiness({
      ...failedReadiness(
        runtimeFailure("request_rejected", "review_configuration"),
      ),
      credentials: [{ key: "openrouter_api_key", present: true }],
    });

    expect(providerRecoveryAction(entry, t)).toBe(
      t("settings.providerReadiness.recovery.reviewConfiguration"),
    );
  });

  it("covers every closed runtime code in the failure table", () => {
    const covered = new Set(FAILURE_CASES.map((c) => c.name));
    for (const code of RUNTIME_DIAGNOSTIC_CONTRACT.error_codes) {
      expect(covered).toContain(`runtime/${code}`);
    }
  });

  it("decides recovery from the typed cause alone, never from message text", () => {
    // The legacy `Credential rejected (401):` prose used to classify an entry
    // as a rejected credential. It is inert now: only the diagnostic decides.
    const entry = {
      ...readiness({
        credentials: [{ key: "deepgram_api_key", present: true }],
        ...failedReadiness(runtimeFailure("request_rejected")),
      }),
      message: "Credential rejected (401): Deepgram returned HTTP 401",
    } as unknown as ProviderReadiness;

    expect(providerRecoveryAction(entry, t)).toBe(
      t("settings.providerReadiness.recovery.runChecksAgain"),
    );
    expect(providerReadinessMessage(entry, t)).toBe(
      t("runtimeDiagnostics.runtime.request_rejected"),
    );
  });

  it("gives each typed provider-auth cause the recovery copy for it", () => {
    const recovery = (diagnostic: ProviderReadiness["diagnostic"]) =>
      providerRecoveryAction(
        readiness({
          provider_id: "llm.openrouter",
          status: "error",
          note: { kind: "failed" },
          diagnostic,
          credentials: [{ key: "openrouter_api_key", present: true }],
        }),
        t,
      );
    const keyAdvice = /saved key|saved credential|replace/i;

    expect(
      recovery(runtimeFailure("credential_rejected", "reenter_credential")),
    ).toMatch(/saved credential was rejected/i);
    // Replacing the key fixes neither an expired credential nor a valid key
    // the provider will not let do this.
    expect(
      recovery(runtimeFailure("credential_expired", "refresh_credential")),
    ).toMatch(/expired/i);
    expect(
      recovery(runtimeFailure("permission_denied", "review_configuration")),
    ).toMatch(/account permissions or plan/i);
    // A depleted quota or balance (HTTP 402) points at the plan, not at the
    // model, region or endpoint.
    expect(
      recovery(runtimeFailure("capacity_exhausted", "review_configuration")),
    ).toBe(t("runtimeDiagnostics.runtime.capacity_exhausted"));
    // A rejected request, a rate limit, an outage, a timeout and a network
    // failure are not evidence that the saved key itself is wrong.
    expect(
      recovery(runtimeFailure("request_rejected", "review_configuration")),
    ).toMatch(/model, region, or endpoint/i);
    expect(
      recovery(runtimeFailure("rate_limited", "retry_after_delay", true)),
    ).toMatch(/in a few minutes/i);
    expect(
      recovery(runtimeFailure("provider_unavailable", "retry", true)),
    ).toMatch(/in a few minutes/i);
    expect(recovery(runtimeFailure("timeout", "retry", true))).toMatch(
      /in a few minutes/i,
    );
    expect(
      recovery(runtimeFailure("network_unreachable", "check_network", true)),
    ).toMatch(/network connection/i);
    for (const diagnostic of [
      runtimeFailure("request_rejected", "review_configuration"),
      runtimeFailure("rate_limited", "retry_after_delay", true),
      runtimeFailure("provider_unavailable", "retry", true),
      runtimeFailure("timeout", "retry", true),
      runtimeFailure("network_unreachable", "check_network", true),
      runtimeFailure("invalid_configuration", "review_configuration"),
    ]) {
      expect(recovery(diagnostic)).not.toMatch(keyAdvice);
    }
    // A locked store says to unlock it, not to replace the key.
    expect(recovery(credentialStoreFailure("locked", "unlock_store"))).toMatch(
      /unlock the credential store/i,
    );
  });

  it("applies one rule to ASR, TTS, LLM and AWS entries", () => {
    for (const providerId of [
      "asr.deepgram",
      "tts.deepgram_aura",
      "llm.openrouter",
      "asr.aws_transcribe",
      "llm.aws_bedrock",
    ]) {
      const rejected = readiness({
        provider_id: providerId,
        credentials: [{ key: "some_credential", present: true }],
        ...failedReadiness(
          runtimeFailure("credential_rejected", "reenter_credential"),
        ),
      });
      expect(providerRecoveryAction(rejected, t)).toBe(
        t("settings.providerReadiness.recovery.credentialRejected"),
      );
    }
  });

  it("renders the dedicated recovery banner for a rejected credential instead of the generic error copy", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          credentials: [{ key: "deepgram_api_key", present: true }],
          ...failedReadiness(
            runtimeFailure("credential_rejected", "reenter_credential"),
          ),
        })}
        loading={false}
        t={t}
      />,
    );

    // The readiness sentence says the provider rejected the credential; the
    // recovery banner is the one carrying the replace-the-key step.
    expect(
      screen.getByText(/the saved credential was rejected/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/replace the saved key or adjust provider settings/i),
    ).not.toBeInTheDocument();
  });

  it.each([
    ["en", "permission_denied", /denied access to this operation/i],
    ["en", "credential_rejected", /rejected the credential/i],
    ["pt", "permission_denied", /negou o acesso a esta operação/i],
    ["pt", "credential_rejected", /rejeitou a credencial/i],
  ] as const)("presents a typed readiness failure in %s (%s) as a sentence, never its token", (locale, code, sentence) => {
    const { container } = render(
      <ProviderReadinessPanel
        entry={readiness({
          provider_id: "llm.openrouter",
          status: "error",
          note: { kind: "failed" },
          diagnostic: {
            kind: "runtime",
            detail: {
              code,
              retryable: false,
              recovery_action:
                code === "credential_rejected"
                  ? "reenter_credential"
                  : "review_configuration",
              context: { operation: "provider_readiness", transport: "http" },
            },
          },
          credentials: [{ key: "openrouter_api_key", present: true }],
        })}
        loading={false}
        t={i18n.getFixedT(locale)}
      />,
    );

    // Once: a sentence that is its own next step is not repeated as one.
    expect(screen.getAllByText(sentence)).toHaveLength(1);
    expect(screen.getByRole("status")).toHaveTextContent(sentence);
    expect(container).not.toHaveTextContent("runtime/");
    expect(container).not.toHaveTextContent(code);
  });

  it("guides unchecked saved credentials toward validation checks", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({ checked_at: 1_700_000_000_000 })}
        credentialPresence={{
          deepgram_api_key: {
            key: "deepgram_api_key",
            present: true,
            source: "credentials_yaml",
          },
        }}
        loading={false}
        t={t}
      />,
    );

    const status = screen.getByRole("status");
    expect(status).toHaveTextContent(/Unchecked/i);
    expect(status).toHaveTextContent(/Ready to check with saved credentials/i);
    expect(screen.getByText(/run checks to validate/i)).toBeInTheDocument();
    fireEvent.click(screen.getByText(/details/i));
    expect(screen.getByText(/last checked/i)).toBeInTheDocument();
    expect(screen.getByText("deepgram_api_key")).toBeInTheDocument();
    expect(screen.getByText(/credentials\.yaml/i)).toBeInTheDocument();
    expect(screen.getByText(/present/i)).toBeInTheDocument();
    expect(status).not.toHaveTextContent(/last checked/i);
    expect(status).not.toHaveTextContent(/deepgram_api_key/i);
    expect(status).not.toHaveTextContent(/credentials\.yaml/i);
    expect(screen.queryByText(/sk-/i)).not.toBeInTheDocument();
  });

  it("renders keychain and fallback credential source labels without raw values", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          credentials: [
            { key: "openrouter_api_key", present: true },
            { key: "deepgram_api_key", present: true },
            { key: "assemblyai_api_key", present: true },
          ],
        })}
        credentialPresence={{
          openrouter_api_key: {
            key: "openrouter_api_key",
            present: true,
            source: "os_keychain",
          },
          deepgram_api_key: {
            key: "deepgram_api_key",
            present: true,
            source: "imported_file",
          },
          assemblyai_api_key: {
            key: "assemblyai_api_key",
            present: true,
            source: "file_fallback",
          },
        }}
        loading={false}
        t={t}
      />,
    );

    fireEvent.click(screen.getByText(/details/i));
    expect(screen.getByText(/OS keychain/i)).toBeInTheDocument();
    expect(
      screen.getByText(/Imported from credentials\.yaml/i),
    ).toBeInTheDocument();
    expect(screen.getByText(/File fallback/i)).toBeInTheDocument();
    expect(screen.queryByText(/os_keychain/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/sk-/i)).not.toBeInTheDocument();
  });

  it("renders the credentials.yaml override source label without raw values", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          credentials: [{ key: "deepgram_api_key", present: true }],
        })}
        credentialPresence={{
          deepgram_api_key: {
            key: "deepgram_api_key",
            present: true,
            source: "file_override",
          },
        }}
        loading={false}
        t={t}
      />,
    );

    fireEvent.click(screen.getByText(/details/i));
    expect(screen.getByText(/credentials\.yaml override/i)).toBeInTheDocument();
    // The raw backend source string must never reach the DOM (label only).
    expect(screen.queryByText(/file_override/)).not.toBeInTheDocument();
    expect(screen.queryByText(/sk-/i)).not.toBeInTheDocument();
  });

  it("renders unknown source for a present readiness credential without presence metadata", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          credentials: [{ key: "deepgram_api_key", present: true }],
        })}
        credentialPresence={{}}
        loading={false}
        t={t}
      />,
    );

    fireEvent.click(screen.getByText(/details/i));
    const credentialRow = screen.getByText("deepgram_api_key").closest("dd");
    expect(credentialRow).not.toBeNull();
    expect(
      within(credentialRow as HTMLElement).getByText(/present/i),
    ).toBeInTheDocument();
    expect(screen.getByText(/Unknown source/i)).toBeInTheDocument();
    expect(screen.queryByText(/credentials\.yaml/i)).not.toBeInTheDocument();
  });

  it("renders missing for a missing readiness credential without presence metadata", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          credentials: [{ key: "deepgram_api_key", present: false }],
        })}
        credentialPresence={{}}
        loading={false}
        t={t}
      />,
    );

    fireEvent.click(screen.getByText(/details/i));
    const credentialRow = screen.getByText("deepgram_api_key").closest("dd");
    expect(credentialRow).not.toBeNull();
    expect(
      within(credentialRow as HTMLElement).getAllByText(/missing/i),
    ).toHaveLength(2);
    expect(screen.queryByText(/credentials\.yaml/i)).not.toBeInTheDocument();
  });

  it("does not suggest validation checks for providers without a health command", () => {
    const descriptor = GENERATED_PROVIDER_REGISTRY.find(
      (provider) => provider.id === "asr.gladia",
    );
    const entry = readiness({
      provider_id: "asr.gladia",
      note: { kind: "no_automatic_probe" },
      automatic_probe_available: false,
      credentials: [{ key: "gladia_api_key", present: true }],
    });

    expect(providerRecoveryAction(entry, t, descriptor)).toBeNull();
  });

  it("does not suggest unsupported automatic checks for Gemini Vertex saved credentials", () => {
    const descriptor = GENERATED_PROVIDER_REGISTRY.find(
      (provider) => provider.id === "realtime_agent.gemini_live",
    );
    const entry = readiness({
      provider_id: "realtime_agent.gemini_live",
      note: { kind: "vertex_not_probed" },
      automatic_probe_available: false,
      credentials: [{ key: "google_service_account_path", present: true }],
    });

    render(
      <ProviderReadinessPanel
        entry={entry}
        descriptor={descriptor}
        loading={false}
        t={t}
      />,
    );

    expect(providerRecoveryAction(entry, t, descriptor)).toBeNull();
    expect(
      screen.queryByText(/run checks to validate/i),
    ).not.toBeInTheDocument();
    expect(screen.getByText(/not probed automatically/i)).toBeInTheDocument();
    expect(screen.queryByText(/sk-/i)).not.toBeInTheDocument();
  });

  it("does not show credential recovery for credentialless local providers", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          provider_id: "llm.mistralrs",
          status: "unchecked",
          note: { kind: "local_models_managed" },
          credentials: [],
        })}
        loading={false}
        t={t}
      />,
    );

    expect(screen.queryByText(/^Next/i)).not.toBeInTheDocument();
  });

  it("does not ask unchecked provider modes to add keys unless credentials are required", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          provider_id: "asr.aws_transcribe",
          status: "unchecked",
          note: { kind: "ready_to_check" },
          credentials: [
            { key: "aws_access_key", present: false },
            { key: "aws_secret_key", present: false },
          ],
        })}
        loading={false}
        t={t}
      />,
    );

    expect(screen.queryByText(/^Next/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/add a key/i)).not.toBeInTheDocument();
  });

  it("renders non-secret local runtime readiness details", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          provider_id: "asr.moonshine",
          note: {
            kind: "local_models",
            ready: 1,
            total: 3,
            missing: [],
            invalid: [],
            planned: true,
          },
          credentials: [],
          runtime: {
            status: "runtime_unavailable",
            required_feature: null,
            runtime_version: null,
            model_id: "moonshine-small-streaming-en",
          },
        })}
        loading={false}
        t={t}
      />,
    );

    fireEvent.click(screen.getByText(/details/i));
    expect(screen.getByText(/runtime unavailable/i)).toBeInTheDocument();
    expect(
      screen.getByText(/the local runtime is not available yet/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/sk-/i)).not.toBeInTheDocument();
  });

  it("keeps the live status region to concise readiness text", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          provider_id: "tts.deepgram_aura",
          status: "ready",
          note: { kind: "verified" },
          stale: true,
          voice_catalog: [
            {
              id: "aura-asteria-en",
              display_name: "Asteria",
              is_default: true,
            },
            {
              id: "aura-zeus-en",
              display_name: "Zeus",
              is_default: false,
            },
          ],
        })}
        loading={true}
        t={t}
      />,
    );

    const status = screen.getByRole("status");
    expect(status).toHaveAttribute("aria-live", "polite");
    expect(status).toHaveAttribute("aria-atomic", "true");
    expect(status).toHaveAttribute("aria-busy", "true");
    // Tone-law completion (settings T3, audio-graph-9d2b — folds in seed
    // 73bf): this panel always describes the ACTIVE provider (every caller
    // passes the currently-selected provider's readiness), so `stale: true`
    // demotes the whole label to "Unchecked" instead of rendering "Ready"
    // with a merely appended stale sentence — the previous version of this
    // test asserted exactly that anti-pattern.
    expect(status).not.toHaveTextContent(/Ready/i);
    expect(status).toHaveTextContent(/Unchecked/i);
    expect(status).toHaveTextContent(/Checking/i);
    expect(status).toHaveTextContent(/Provider check passed/i);
    expect(status).toHaveTextContent(/Cached result may be stale/i);
    expect(status).toHaveTextContent(/Catalog: 2 voices/i);
  });

  it("renders typed voice catalog summaries without calling them models", () => {
    const entry = readiness({
      provider_id: "tts.deepgram_aura",
      status: "ready",
      note: { kind: "verified" },
      voice_catalog: [
        {
          id: "aura-asteria-en",
          display_name: "Asteria",
          is_default: true,
        },
        {
          id: "aura-zeus-en",
          display_name: "Zeus",
          is_default: false,
        },
      ],
    });

    render(<ProviderReadinessPanel entry={entry} loading={false} t={t} />);

    expect(providerCatalogSummary(entry)).toEqual({
      count: 2,
      kind: "voices",
    });
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent(/Ready/i);
    expect(status).toHaveTextContent(/Provider check passed/i);
    expect(status).toHaveTextContent(/Catalog: 2 voices/i);
    expect(status).not.toHaveTextContent(/2 models/i);
  });

  it("renders required-not-wired roadmap auth without implying no credentials", () => {
    const descriptor = GENERATED_PROVIDER_REGISTRY.find(
      (provider) => provider.id === "asr.xai_grok_stt",
    );
    if (!descriptor) throw new Error("xAI watch descriptor missing");

    render(
      <ProviderReadinessPanel
        entry={null}
        descriptor={descriptor}
        loading={false}
        t={t}
      />,
    );

    expect(
      screen.getByText(/auth required; credential schema not wired/i),
    ).toBeInTheDocument();
    expect(screen.getByText(/watch candidate/i)).toBeInTheDocument();
    expect(
      screen.queryByText(/no credential required/i),
    ).not.toBeInTheDocument();
  });

  it("localizes the roadmap label and programmatically associates the not-selectable reason", () => {
    const descriptor = GENERATED_PROVIDER_REGISTRY.find(
      (provider) => provider.id === "asr.xai_grok_stt",
    );
    if (!descriptor) throw new Error("xAI watch descriptor missing");

    render(
      <ProviderReadinessPanel
        entry={null}
        descriptor={descriptor}
        loading={false}
        t={t}
      />,
    );

    // Roadmap label and status route through i18n (no hardcoded EN literal).
    const roadmapLabel = screen.getByText(
      t("settings.providerReadiness.roadmap"),
    );
    expect(roadmapLabel.tagName).toBe("DT");
    expect(screen.getByText(/watch candidate/i)).toBeInTheDocument();

    // The free-form backend reason is rendered with the localized label and is
    // programmatically associated with the roadmap value via aria-describedby.
    expect(
      screen.getByText(
        t("settings.providerReadiness.notSelectableReasonLabel"),
      ),
    ).toBeInTheDocument();
    const reasonText = screen.getByText(
      /credential schema and runtime adapter are not wired/i,
    );
    const describedNode = reasonText.closest("[id]") as HTMLElement;
    expect(describedNode.id).toBeTruthy();
    const dd = roadmapLabel.nextElementSibling as HTMLElement;
    expect(dd.tagName).toBe("DD");
    expect(dd).toHaveAttribute("aria-describedby", describedNode.id);
  });

  it("renders data-boundary classes and unknown policy status without overclaiming", () => {
    // asr.soniox is a cloud provider that does NOT (yet) carry a sourced
    // privacy `policy_url` in the registry (seed audio-graph-fee1 only filled in
    // the providers with a verifiable official policy URL — Deepgram/OpenAI/AWS/
    // AssemblyAI). It therefore still exercises the "No policy URL recorded"
    // fallback without overclaiming. (Deepgram now has a sourced URL — see the
    // companion test below.)
    const descriptor = GENERATED_PROVIDER_REGISTRY.find(
      (provider) => provider.id === "asr.soniox",
    );
    if (!descriptor) throw new Error("Soniox descriptor missing");

    render(
      <ProviderReadinessPanel
        entry={readiness({
          provider_id: "asr.soniox",
          status: "ready",
          note: { kind: "verified" },
        })}
        descriptor={descriptor}
        loading={false}
        t={t}
      />,
    );

    const status = screen.getByRole("status");
    expect(status).toHaveTextContent(/Ready/i);
    expect(status).toHaveTextContent(/Provider check passed/i);
    expect(status).not.toHaveTextContent(/Vendor cloud/i);
    expect(status).not.toHaveTextContent(/No policy URL recorded/i);
    expect(screen.getByText(/Vendor cloud/i)).toBeInTheDocument();
    // Unknown-policy provider falls back without fabricating a policy claim.
    expect(screen.getByText(/No policy URL recorded/i)).toBeInTheDocument();
    expect(status).not.toHaveTextContent(/retained for/i);
    expect(status).not.toHaveTextContent(/used for training/i);
    expect(status).not.toHaveTextContent(/sk-/i);
  });

  it("renders a sourced provider policy URL instead of the fallback (fee1)", () => {
    // Deepgram gained an official, source-dated policy URL via seed
    // audio-graph-fee1, so the panel must surface the real link rather than the
    // "No policy URL recorded" fallback — proving sourced metadata is shown.
    const descriptor = GENERATED_PROVIDER_REGISTRY.find(
      (provider) => provider.id === "asr.deepgram",
    );
    if (!descriptor) throw new Error("Deepgram descriptor missing");
    expect(descriptor.privacy.policy_url).toBeTruthy();

    render(
      <ProviderReadinessPanel
        entry={readiness({
          provider_id: "asr.deepgram",
          status: "ready",
          note: { kind: "verified" },
        })}
        descriptor={descriptor}
        loading={false}
        t={t}
      />,
    );

    expect(
      screen.queryByText(/No policy URL recorded/i),
    ).not.toBeInTheDocument();
    // The panel renders the sourced URL as text in the policy <dd> (plain-text
    // today; rendering it as a clickable <a> is a separate UX follow-up).
    const url = descriptor.privacy.policy_url as string;
    expect(screen.getByText(url)).toBeInTheDocument();
  });

  it("updates the aria-live status region when readiness changes without leaking a credential value", () => {
    // A sentinel secret-shaped token planted in the credential key. The panel
    // must never echo a credential identifier (let alone a value) into the
    // role=status live region — that region carries only the localized status
    // label, the readiness message, and the catalog summary. Using a key that
    // looks like an API secret makes the no-leak assertions below non-vacuous:
    // if the live region ever started rendering credentials, this token would
    // surface and the test would fail.
    const SECRET_SENTINEL = "sk-live-deadbeef-never-render";

    const { rerender } = render(
      <ProviderReadinessPanel
        entry={readiness({
          status: "missing_credentials",
          note: { kind: "missing_credentials", keys: ["deepgram_api_key"] },
          credentials: [{ key: SECRET_SENTINEL, present: false }],
        })}
        loading={false}
        t={t}
      />,
    );

    // The live region exists and announces the initial readiness. getByRole
    // throws if the role=status region is removed, so this guards its presence.
    const status = screen.getByRole("status");
    expect(status).toHaveAttribute("aria-live", "polite");
    expect(status).toHaveTextContent(/Missing key/i);
    expect(status).toHaveTextContent(
      /Missing saved credential: deepgram_api_key/i,
    );
    // The credential identifier never reaches the live region (no secret leak).
    expect(status).not.toHaveTextContent(SECRET_SENTINEL);
    expect(status).not.toHaveTextContent(/sk-/i);

    // Provider readiness changes (e.g. after a successful health check).
    rerender(
      <ProviderReadinessPanel
        entry={readiness({
          status: "ready",
          note: { kind: "verified" },
          credentials: [{ key: SECRET_SENTINEL, present: true }],
        })}
        loading={false}
        t={t}
      />,
    );

    // The SAME live region updates in place with the new status + message,
    // and the stale "missing" copy is gone.
    const updatedStatus = screen.getByRole("status");
    expect(updatedStatus).toHaveAttribute("aria-live", "polite");
    expect(updatedStatus).toHaveTextContent(/Ready/i);
    expect(updatedStatus).toHaveTextContent(/Provider check passed/i);
    expect(updatedStatus).not.toHaveTextContent(/Missing key/i);
    expect(updatedStatus).not.toHaveTextContent(/Missing saved credential/i);
    // Still no credential value/identifier leak after the transition.
    expect(updatedStatus).not.toHaveTextContent(SECRET_SENTINEL);
    expect(updatedStatus).not.toHaveTextContent(/sk-/i);
  });

  describe("readiness copy from closed fields", () => {
    const LOCALES = ["en", "pt"] as const;
    const renderIn = (
      locale: (typeof LOCALES)[number],
      entry: ProviderReadiness,
    ) => {
      const view = render(
        <ProviderReadinessPanel
          entry={entry}
          loading={false}
          t={i18n.getFixedT(locale)}
        />,
      );
      return {
        view,
        message: view.container.querySelector(
          ".settings-provider-readiness__message",
        ),
        recovery: view.container.querySelector(
          ".settings-provider-readiness__recovery",
        ),
      };
    };

    it.each(
      READINESS_NOTE_CASES.map((c) => [JSON.stringify(c.note), c] as const),
    )("renders the note %s as localized copy, not a raw token", (_label, c) => {
      for (const locale of LOCALES) {
        const { view, message } = renderIn(locale, readiness({ note: c.note }));
        expect(message?.textContent).toBe(c[locale]);
        expect(view.container.textContent).not.toMatch(
          /runtime\/|credential\//,
        );
        if (c.note.kind.includes("_")) {
          expect(view.container.textContent).not.toContain(c.note.kind);
        }
        view.unmount();
      }
    });

    it.each(
      FAILURE_CASES.map((c) => [c.name, c] as const),
    )("presents %s with its own copy and recovery action", (_name, c) => {
      for (const locale of LOCALES) {
        const entry = readiness({
          provider_id: "llm.openrouter",
          credentials: [{ key: "openrouter_api_key", present: true }],
          ...failedReadiness(c.diagnostic),
        });
        const { view, message, recovery } = renderIn(locale, entry);
        expect(message?.textContent).toBe(c.message[locale]);
        if (c.recovery) {
          expect(recovery).toHaveTextContent(c.recovery[locale]);
        } else {
          // The sentence above is already the next step: shown once.
          expect(recovery).toBeNull();
          expect(screen.getAllByText(c.message[locale])).toHaveLength(1);
        }
        expect(providerRecoveryAction(entry, i18n.getFixedT(locale))).toBe(
          c.recovery?.[locale] ?? c.message[locale],
        );
        // The cause is not always an HTTP 401 (an STS or store failure).
        expect(view.container.textContent).not.toContain("(401)");
        expect(view.container.textContent).not.toMatch(
          /runtime\/|credential\//,
        );
        view.unmount();
      }
    });

    it("counts the models of a passed check, pluralized, but never calls voices models", () => {
      const passed = (
        overrides: Partial<ProviderReadiness>,
        locale: (typeof LOCALES)[number] = "en",
      ) =>
        providerReadinessMessage(
          readiness({
            status: "ready",
            note: { kind: "verified" },
            ...overrides,
          }),
          i18n.getFixedT(locale),
        );

      expect(passed({ model_count: 12 })).toBe(
        "Provider check passed: 12 models available.",
      );
      expect(passed({ model_count: 1 })).toBe(
        "Provider check passed: 1 model available.",
      );
      expect(passed({ model_count: 12 }, "pt")).toBe(
        "A verificação do provedor foi aprovada: 12 modelos disponíveis.",
      );
      expect(passed({ model_count: 1 }, "pt")).toBe(
        "A verificação do provedor foi aprovada: 1 modelo disponível.",
      );
      expect(passed({})).toBe("Provider check passed.");
      expect(
        passed({
          model_count: 2,
          voice_catalog: [
            {
              id: "aura-asteria-en",
              display_name: "Asteria",
              is_default: true,
            },
            { id: "aura-zeus-en", display_name: "Zeus", is_default: false },
          ],
        }),
      ).toBe("Provider check passed.");
    });

    it("degrades an unknown future note kind to the localized status label", () => {
      const future = readiness({
        status: "unchecked",
        note: {
          kind: "from_a_newer_backend",
        } as unknown as ProviderReadiness["note"],
      });
      expect(providerReadinessMessage(future, t)).toBe(
        t("settings.providerReadiness.status.unchecked"),
      );
    });

    it.each([
      [
        { status: "feature_missing", required_feature: "asr-moonshine" },
        "This build does not include the asr-moonshine runtime feature.",
      ],
      [
        { status: "feature_missing" },
        "This build does not include a required runtime feature.",
      ],
      [
        { status: "model_missing", model_id: "moonshine-tiny-en" },
        "The local model files for moonshine-tiny-en are missing or incomplete.",
      ],
      [
        { status: "model_missing" },
        "No complete local model is installed yet.",
      ],
      [
        { status: "runtime_unavailable" },
        "The local runtime is not available yet.",
      ],
      [
        { status: "load_failed", model_id: "moonshine-tiny-en" },
        "The local runtime could not load moonshine-tiny-en.",
      ],
      [
        { status: "load_failed" },
        "The local runtime could not load the model.",
      ],
      [
        {
          status: "healthy",
          model_id: "moonshine-tiny-en",
          runtime_version: "1.2.3",
        },
        "The local runtime loaded moonshine-tiny-en successfully. Runtime version 1.2.3.",
      ],
      [
        { status: "healthy" },
        "The local runtime loaded the model successfully.",
      ],
    ] as ReadonlyArray<
      readonly [ProviderRuntimeReadiness, string]
    >)("renders the runtime check %j from its closed fields", (runtime, sentence) => {
      expect(providerRuntimeMessage(runtime, t)).toBe(sentence);
      // Both locales render real copy, never an echoed key.
      const pt = providerRuntimeMessage(runtime, i18n.getFixedT("pt"));
      expect(pt).not.toBe(sentence);
      expect(pt).not.toMatch(/settings\.providerReadiness/);
    });
  });
});
