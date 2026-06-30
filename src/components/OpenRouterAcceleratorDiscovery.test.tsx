import { fireEvent, render, screen, within } from "@testing-library/react";
import type { TFunction } from "i18next";
import { describe, expect, it, vi } from "vitest";
import type {
  OpenRouterEndpoint,
  OpenRouterModelEndpoints,
  OpenRouterProvider,
} from "../types";
import type { AcceleratorPreset } from "../utils/openrouterCatalog";
import OpenRouterAcceleratorDiscovery, {
  type OpenRouterAcceleratorDiscoveryProps,
} from "./OpenRouterAcceleratorDiscovery";

// A `t` that returns the i18n KEY (with {{vars}} interpolated) so assertions can
// target stable keys without coupling to copy. This mirrors how the real i18n
// resolves placeholders, which the component relies on for the apply hint etc.
const t = ((key: string, vars?: Record<string, unknown>) => {
  if (!vars) return key;
  return Object.entries(vars).reduce(
    (acc, [name, value]) => acc.replace(`{{${name}}}`, String(value)),
    key,
  );
}) as TFunction;

function provider(
  slug: string,
  name: string,
  overrides: Partial<OpenRouterProvider> = {},
): OpenRouterProvider {
  return {
    slug,
    name,
    privacy_policy_url: `https://example.com/${slug}/privacy`,
    terms_of_service_url: `https://example.com/${slug}/tos`,
    status_page_url: null,
    headquarters: null,
    datacenters: [],
    ...overrides,
  };
}

function endpoint(
  overrides: Partial<OpenRouterEndpoint> = {},
): OpenRouterEndpoint {
  return {
    name: "endpoint",
    model_id: "meta-llama/llama-3.1-70b",
    model_name: "Llama 3.1 70B",
    context_length: 131072,
    pricing: { prompt: "0.0000006", completion: "0.0000006" },
    provider_name: "Cerebras",
    tag: "cerebras/fp16",
    quantization: "fp16",
    max_completion_tokens: null,
    max_prompt_tokens: null,
    supported_parameters: ["tools"],
    uptime_last_30m: 0.999,
    uptime_last_5m: null,
    uptime_last_1d: null,
    supports_implicit_caching: null,
    latency_last_30m: { p50: 0.15 },
    throughput_last_30m: { p50: 2100 },
    status: null,
    ...overrides,
  };
}

function modelEndpoints(
  endpoints: OpenRouterEndpoint[],
): OpenRouterModelEndpoints {
  return {
    id: "meta-llama/llama-3.1-70b",
    name: "Llama 3.1 70B",
    created: null,
    description: null,
    architecture: null,
    endpoints,
  };
}

function renderPanel(
  overrides: Partial<OpenRouterAcceleratorDiscoveryProps> = {},
) {
  const onDiscover = vi.fn();
  const onApplyPreset = vi.fn();
  const onSelectPreset = vi.fn();
  const props: OpenRouterAcceleratorDiscoveryProps = {
    t,
    endpoints: null,
    providers: null,
    modelId: "meta-llama/llama-3.1-70b",
    loading: false,
    error: null,
    credentialAvailable: true,
    selectedPreset: "low_latency",
    appliedPreset: null,
    onSelectPreset,
    onDiscover,
    onApplyPreset,
    ...overrides,
  };
  render(<OpenRouterAcceleratorDiscovery {...props} />);
  return { onDiscover, onApplyPreset, onSelectPreset, props };
}

describe("OpenRouterAcceleratorDiscovery: discovery", () => {
  it("shows the idle prompt and triggers the saved-key fetch on Discover", () => {
    const { onDiscover } = renderPanel();

    // Before any fetch, the panel invites discovery (no hardcoded table).
    expect(
      screen.getByText("settings.acceleratorDiscovery.idle"),
    ).toBeInTheDocument();

    const button = screen.getByRole("button", {
      name: "settings.acceleratorDiscovery.discover",
    });
    expect(button).toBeEnabled();
    fireEvent.click(button);
    expect(onDiscover).toHaveBeenCalledTimes(1);
  });

  it("disables Discover when no credential is available or no model is picked", () => {
    const { onDiscover } = renderPanel({
      credentialAvailable: false,
    });
    fireEvent.click(
      screen.getByRole("button", {
        name: "settings.acceleratorDiscovery.discover",
      }),
    );
    expect(onDiscover).not.toHaveBeenCalled();

    renderPanel({ modelId: "  " });
    // The need-model hint surfaces when no model is selected.
    expect(
      screen.getByText("settings.acceleratorDiscovery.needModel"),
    ).toBeInTheDocument();
  });

  it("renders a ranked accelerator table from the saved-key catalog payloads with source labels", () => {
    renderPanel({
      endpoints: modelEndpoints([
        endpoint({
          provider_name: "Cerebras",
          tag: "cerebras/fp16",
          latency_last_30m: { p50: 0.15 },
        }),
        endpoint({
          provider_name: "Groq",
          tag: "groq/fp8",
          quantization: "fp8",
          latency_last_30m: { p50: 0.25 },
          throughput_last_30m: { p50: 1200 },
        }),
      ]),
      providers: [provider("cerebras", "Cerebras"), provider("groq", "Groq")],
      selectedPreset: "low_latency",
    });

    const table = screen.getByRole("table");
    const rows = within(table).getAllByRole("row");
    // header row + 2 data rows
    expect(rows).toHaveLength(3);
    // Low-latency ranking puts Cerebras (150ms) ahead of Groq (250ms).
    const firstDataRow = rows[1];
    expect(within(firstDataRow).getByText("Cerebras")).toBeInTheDocument();
    expect(within(firstDataRow).getByText("cerebras")).toBeInTheDocument();
    // Source label is a verifiable policy link from the catalog (not fabricated).
    const policyLink = within(firstDataRow).getByRole("link", {
      name: "settings.acceleratorDiscovery.source.catalog",
    });
    expect(policyLink).toHaveAttribute(
      "href",
      "https://example.com/cerebras/privacy",
    );
  });
});

describe("OpenRouterAcceleratorDiscovery: missing metadata + empty states", () => {
  it("renders the empty state when the discovered model exposes no endpoints", () => {
    renderPanel({ endpoints: modelEndpoints([]), providers: [] });
    expect(
      screen.getByText("settings.acceleratorDiscovery.empty"),
    ).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("surfaces the error state with the fetch error message", () => {
    // The error copy interpolates {{error}}; pass a t that echoes the message so
    // the rendered alert carries the underlying fetch failure for the user.
    const echoT = ((key: string, vars?: Record<string, unknown>) =>
      vars && "error" in vars ? `${key}: ${vars.error}` : key) as TFunction;
    render(
      <OpenRouterAcceleratorDiscovery
        t={echoT}
        endpoints={null}
        providers={null}
        modelId="meta-llama/llama-3.1-70b"
        loading={false}
        error="network down"
        credentialAvailable
        selectedPreset="low_latency"
        appliedPreset={null}
        onSelectPreset={vi.fn()}
        onDiscover={vi.fn()}
        onApplyPreset={vi.fn()}
      />,
    );
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("settings.acceleratorDiscovery.error");
    expect(alert).toHaveTextContent("network down");
  });

  it("falls back to a derived slug + 'policy unknown' label when provider metadata is missing", () => {
    // Provider absent from /providers (missing-metadata fallback): the endpoint
    // still renders, the slug is derived from the name, and the source label is
    // the derived-policy-unknown variant — never a fabricated policy link.
    renderPanel({
      endpoints: modelEndpoints([
        endpoint({ provider_name: "New Accel Inc", tag: "new-accel/fp16" }),
      ]),
      providers: [],
      selectedPreset: "low_latency",
    });

    const table = screen.getByRole("table");
    const dataRow = within(table).getAllByRole("row")[1];
    expect(within(dataRow).getByText("New Accel Inc")).toBeInTheDocument();
    expect(within(dataRow).getByText("new-accel-inc")).toBeInTheDocument();
    // No policy link — provenance is the derived (unknown) variant.
    expect(within(dataRow).queryByRole("link")).not.toBeInTheDocument();
    expect(
      within(dataRow).getByText("settings.acceleratorDiscovery.source.derived"),
    ).toBeInTheDocument();
  });

  it("shows the privacy-preset empty message when no endpoint has verifiable policy provenance", () => {
    renderPanel({
      endpoints: modelEndpoints([
        endpoint({ provider_name: "Mystery", tag: "mystery/q" }),
      ]),
      // Mystery has no privacy/ToS URL → filtered out of privacy_zdr ranking.
      providers: [
        provider("mystery", "Mystery", {
          privacy_policy_url: null,
          terms_of_service_url: null,
        }),
      ],
      selectedPreset: "privacy_zdr",
    });
    expect(
      screen.getByText("settings.acceleratorDiscovery.noCandidatesForPreset"),
    ).toBeInTheDocument();
    // No applicable slugs for this preset → the Apply button is disabled so the
    // user cannot push an empty order into the routing policy.
    expect(
      screen.getByRole("button", {
        name: /settings\.acceleratorDiscovery\.apply/,
      }),
    ).toBeDisabled();
  });
});

describe("OpenRouterAcceleratorDiscovery: applying a preset", () => {
  it("applies the ranked accelerator order (dynamic, not hardcoded) for the selected preset", () => {
    const { onApplyPreset } = renderPanel({
      endpoints: modelEndpoints([
        endpoint({
          provider_name: "Cerebras",
          tag: "cerebras/fp16",
          latency_last_30m: { p50: 0.15 },
        }),
        endpoint({
          provider_name: "Groq",
          tag: "groq/fp8",
          latency_last_30m: { p50: 0.25 },
        }),
      ]),
      providers: [provider("cerebras", "Cerebras"), provider("groq", "Groq")],
      selectedPreset: "low_latency",
    });

    const applyButton = screen.getByRole("button", {
      name: /settings\.acceleratorDiscovery\.apply/,
    });
    fireEvent.click(applyButton);

    expect(onApplyPreset).toHaveBeenCalledTimes(1);
    // The applied order is derived from the live catalog ranking, deduped — the
    // replacement for the old hardcoded ["cerebras","groq"] default.
    expect(onApplyPreset).toHaveBeenCalledWith("low_latency", [
      "cerebras",
      "groq",
    ]);
  });

  it("changes the active preset when a preset radio is chosen", () => {
    const { onSelectPreset } = renderPanel({
      endpoints: modelEndpoints([endpoint()]),
      providers: [provider("cerebras", "Cerebras")],
      selectedPreset: "low_latency",
    });

    const highThroughput = screen.getByRole("radio", {
      name: "settings.acceleratorDiscovery.presets.highThroughput",
    });
    fireEvent.click(highThroughput);
    expect(onSelectPreset).toHaveBeenCalledWith("high_throughput");
  });

  it("confirms the applied-preset status once a preset is in effect", () => {
    renderPanel({
      endpoints: modelEndpoints([endpoint()]),
      providers: [provider("cerebras", "Cerebras")],
      selectedPreset: "low_latency",
      appliedPreset: "low_latency" as AcceleratorPreset,
    });
    expect(
      screen.getByText(/settings\.acceleratorDiscovery\.applied/),
    ).toBeInTheDocument();
  });
});
