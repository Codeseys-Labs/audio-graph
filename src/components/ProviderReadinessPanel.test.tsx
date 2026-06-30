import { fireEvent, render, screen, within } from "@testing-library/react";
import i18n from "i18next";
import { describe, expect, it } from "vitest";
import { GENERATED_PROVIDER_REGISTRY } from "../generated/providerRegistry";
import type { ProviderReadiness } from "../types";
import ProviderReadinessPanel, {
  providerCatalogSummary,
  providerRecoveryAction,
} from "./ProviderReadinessPanel";
import "../i18n";

const t = i18n.getFixedT("en");

function readiness(
  overrides: Partial<ProviderReadiness> = {},
): ProviderReadiness {
  return {
    provider_id: "asr.deepgram",
    status: "unchecked",
    message: "Ready to check with saved credentials",
    automatic_probe_available: true,
    checked_at: null,
    stale: false,
    credential_epoch: 0,
    credentials: [{ key: "deepgram_api_key", present: true }],
    ...overrides,
  };
}

describe("ProviderReadinessPanel", () => {
  it("guides users to add missing credentials without rendering secret values", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          status: "missing_credentials",
          message: "Missing saved credential(s): deepgram_api_key",
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

  it("guides saved-key errors toward replace or settings repair", () => {
    const entry = readiness({
      status: "error",
      message: "401 Unauthorized",
      credentials: [{ key: "openrouter_api_key", present: true }],
    });

    expect(providerRecoveryAction(entry, t)).toMatch(/replace the saved key/i);
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

  it("provides recovery copy for keychain-unavailable, malformed-import, and file-fallback states in both locales", () => {
    // These three recovery strings (ADR-0019 §"Update UX copy and docs") are
    // surfaced for credential-store recovery flows. Assert each resolves to a
    // real, source-aware localized string (not the raw i18n key, not blank)
    // and never reintroduces "credentials.yaml is the only/primary store"
    // language. credentials.yaml may be NAMED as an import/fallback path.
    const recoveryKeys = [
      "settings.providerReadiness.recovery.keychainUnavailable",
      "settings.providerReadiness.recovery.malformedImportFile",
      "settings.providerReadiness.recovery.fileFallbackMode",
    ];
    for (const locale of ["en", "pt"] as const) {
      const localT = i18n.getFixedT(locale);
      for (const key of recoveryKeys) {
        const value = localT(key);
        expect(value.trim()).not.toBe("");
        expect(value).not.toBe(key);
        expect(value).not.toMatch(/only (credential )?store/i);
        expect(value).not.toMatch(/primary (credential )?store/i);
      }
    }
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
      message: "No automatic health probe is available for this provider yet",
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
      message: "Vertex AI readiness is not probed automatically yet",
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
          message: "Local model readiness is checked by the model manager",
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
          message: "Ready to check with saved credentials",
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
          message: "Local model files ready: 1/3 model option(s).",
          credentials: [],
          runtime: {
            status: "runtime_unavailable",
            message:
              "Moonshine native runtime adapter is not wired yet; provider remains planned and unselectable.",
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
      screen.getByText(/native runtime adapter is not wired/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/sk-/i)).not.toBeInTheDocument();
  });

  it("keeps the live status region to concise readiness text", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          provider_id: "tts.deepgram_aura",
          status: "ready",
          message: "Deepgram Aura key is valid",
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
    expect(status).toHaveTextContent(/Ready/i);
    expect(status).toHaveTextContent(/Checking/i);
    expect(status).toHaveTextContent(/Deepgram Aura key is valid/i);
    expect(status).toHaveTextContent(/Cached result may be stale/i);
    expect(status).toHaveTextContent(/Catalog: 2 voices/i);
  });

  it("renders typed voice catalog summaries without calling them models", () => {
    const entry = readiness({
      provider_id: "tts.deepgram_aura",
      status: "ready",
      message: "Deepgram Aura key is valid",
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
    expect(status).toHaveTextContent(/Deepgram Aura key is valid/i);
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
    const descriptor = GENERATED_PROVIDER_REGISTRY.find(
      (provider) => provider.id === "asr.deepgram",
    );
    if (!descriptor) throw new Error("Deepgram descriptor missing");

    render(
      <ProviderReadinessPanel
        entry={readiness({
          provider_id: "asr.deepgram",
          status: "ready",
          message: "Deepgram key is valid",
        })}
        descriptor={descriptor}
        loading={false}
        t={t}
      />,
    );

    const status = screen.getByRole("status");
    expect(status).toHaveTextContent(/Ready/i);
    expect(status).toHaveTextContent(/Deepgram key is valid/i);
    expect(status).not.toHaveTextContent(/Vendor cloud/i);
    expect(status).not.toHaveTextContent(/Audio, Provider config/i);
    expect(status).not.toHaveTextContent(/Credential auth/i);
    expect(status).not.toHaveTextContent(/No policy URL recorded/i);
    expect(screen.getByText(/Vendor cloud/i)).toBeInTheDocument();
    expect(screen.getByText(/Audio, Provider config/i)).toBeInTheDocument();
    expect(screen.getByText(/Credential auth/i)).toBeInTheDocument();
    expect(screen.getByText(/No policy URL recorded/i)).toBeInTheDocument();
    expect(status).not.toHaveTextContent(/retained for/i);
    expect(status).not.toHaveTextContent(/used for training/i);
    expect(status).not.toHaveTextContent(/sk-/i);
  });
});
