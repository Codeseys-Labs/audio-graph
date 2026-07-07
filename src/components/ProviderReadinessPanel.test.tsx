import { fireEvent, render, screen, within } from "@testing-library/react";
import i18n from "i18next";
import { describe, expect, it } from "vitest";
import { GENERATED_PROVIDER_REGISTRY } from "../generated/providerRegistry";
import type { ProviderReadiness } from "../types";
import ProviderReadinessPanel, {
  isCredentialRejectedReadinessMessage,
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

  it("recognizes the backend's stable credential-rejected (401) prefix", () => {
    // audio-graph-57cc: `isCredentialRejectedReadinessMessage` keys off the
    // exact stable prefix `crate::error::CREDENTIAL_REJECTED_PREFIX` emits
    // (src-tauri/src/error.rs), not a loose "401" substring search — a
    // provider error body could coincidentally contain "401" elsewhere (a
    // request id, a project id, ...), so only the recognized prefix counts.
    expect(
      isCredentialRejectedReadinessMessage(
        "Credential rejected (401): Deepgram returned HTTP 401 Unauthorized",
      ),
    ).toBe(true);
    expect(
      isCredentialRejectedReadinessMessage(
        "provider request id 401-not-a-status-code",
      ),
    ).toBe(false);
    expect(isCredentialRejectedReadinessMessage("401 Unauthorized")).toBe(
      false,
    );
  });

  it("distinguishes a 401 credential rejection from a generic saved-key error", () => {
    // A generic (non-401) error with a saved credential still gets the
    // pre-existing "replace or adjust settings" copy...
    const genericError = readiness({
      status: "error",
      message: "Health check timed out after 10s",
      credentials: [{ key: "openrouter_api_key", present: true }],
    });
    expect(providerRecoveryAction(genericError, t)).toMatch(
      /replace the saved key or adjust provider settings/i,
    );

    // ...but a message carrying the stable 401 prefix gets the dedicated
    // "key rejected — replace it" recovery copy instead of the generic one.
    const credentialRejected = readiness({
      status: "error",
      message: "Credential rejected (401): OpenRouter returned HTTP 401",
      credentials: [{ key: "openrouter_api_key", present: true }],
    });
    const action = providerRecoveryAction(credentialRejected, t);
    expect(action).toMatch(/rejected \(401\)/i);
    expect(action).not.toMatch(
      /replace the saved key or adjust provider settings/i,
    );
  });

  it("renders the dedicated 401 recovery banner instead of the generic error copy", () => {
    render(
      <ProviderReadinessPanel
        entry={readiness({
          status: "error",
          message: "Credential rejected (401): Deepgram returned HTTP 401",
          credentials: [{ key: "deepgram_api_key", present: true }],
        })}
        loading={false}
        t={t}
      />,
    );

    // Scope to the recovery banner's specific copy — the readiness `message`
    // paragraph above it also legitimately echoes "rejected (401)" (it's the
    // raw backend detail), so a loose /rejected \(401\)/i match would be
    // ambiguous (matches both nodes).
    expect(
      screen.getByText(/the saved key was rejected \(401\)/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/replace the saved key or adjust provider settings/i),
    ).not.toBeInTheDocument();
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
          message: "Soniox key is valid",
        })}
        descriptor={descriptor}
        loading={false}
        t={t}
      />,
    );

    const status = screen.getByRole("status");
    expect(status).toHaveTextContent(/Ready/i);
    expect(status).toHaveTextContent(/Soniox key is valid/i);
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
          message: "Deepgram key is valid",
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
          message: "Missing saved credential(s): deepgram_api_key",
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
    expect(status).toHaveTextContent(/Missing saved credential\(s\)/i);
    // The credential identifier never reaches the live region (no secret leak).
    expect(status).not.toHaveTextContent(SECRET_SENTINEL);
    expect(status).not.toHaveTextContent(/sk-/i);

    // Provider readiness changes (e.g. after a successful health check).
    rerender(
      <ProviderReadinessPanel
        entry={readiness({
          status: "ready",
          message: "Deepgram key is valid",
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
    expect(updatedStatus).toHaveTextContent(/Deepgram key is valid/i);
    expect(updatedStatus).not.toHaveTextContent(/Missing key/i);
    expect(updatedStatus).not.toHaveTextContent(/Missing saved credential/i);
    // Still no credential value/identifier leak after the transition.
    expect(updatedStatus).not.toHaveTextContent(SECRET_SENTINEL);
    expect(updatedStatus).not.toHaveTextContent(/sk-/i);
  });
});
