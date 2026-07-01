/**
 * OpenRouter accelerator-catalog VIEW MODEL (seed 61db).
 *
 * Settings today hardcodes the accelerator provider order (`"cerebras, groq"`)
 * to steer OpenRouter's routing at low-latency/high-throughput accelerators.
 * This module replaces that hardcoded list with a NON-SECRET view model derived
 * from the live OpenRouter catalog so Settings can discover accelerator
 * endpoints dynamically.
 *
 * Inputs are the results of the SAVED-KEY catalog commands only:
 *   - `list_openrouter_model_endpoints_cmd` → {@link OpenRouterModelEndpoints}
 *     (the per-model endpoint list: provider name/slug, latency, throughput,
 *     quantization, pricing, context length, supported params, uptime).
 *   - `list_openrouter_providers_cmd` → {@link OpenRouterProvider}[] (provider
 *     metadata: privacy/ToS policy URLs, datacenters, headquarters).
 *
 * This module is PURE: it never calls `invoke`, never reads a plaintext API key,
 * and never holds credentials. The caller fetches the catalog with the
 * saved-key commands and passes the data in. There is NO hardcoded provider as
 * a source of truth — the accelerator list is whatever the catalog exposes.
 *
 * Resilience: OpenRouter omits fields for new models, free models price at zero,
 * provider slugs drift, and the `/providers` and `/endpoints` payloads are
 * loosely coupled. Every normalize step tolerates missing/empty fields and
 * never throws, so a partial catalog still renders a usable (if sparse) view.
 *
 * Provenance honesty: policy URLs are surfaced verbatim from the provider
 * catalog and `null` when the catalog does not expose them — never fabricated.
 */
import type {
  OpenRouterEndpoint,
  OpenRouterModelEndpoints,
  OpenRouterPercentileStats,
  OpenRouterProvider,
} from "../types";

/** Normalized data/privacy policy fields for an accelerator endpoint. */
export interface AcceleratorPrivacyInfo {
  /** Verbatim from the provider catalog; `null` when not exposed (unknown). */
  privacyPolicyUrl: string | null;
  /** Verbatim from the provider catalog; `null` when not exposed (unknown). */
  termsOfServiceUrl: string | null;
  /** Verbatim status-page URL when the catalog exposes it. */
  statusPageUrl: string | null;
  /** Provider headquarters when the catalog exposes it (else `null`). */
  headquarters: string | null;
  /** Datacenter region codes the provider advertises (possibly empty). */
  datacenters: string[];
  /**
   * Whether the endpoint is Zero-Data-Retention. OpenRouter does not expose a
   * per-endpoint ZDR boolean in the public endpoint payload, so this stays
   * `null` (unknown) unless a future field is wired — never guessed.
   */
  zeroDataRetention: boolean | null;
}

/** One normalized accelerator endpoint row in the view model. */
export interface AcceleratorEndpointView {
  /** Provider display name (e.g. "Cerebras"); falls back to the slug/tag. */
  providerName: string;
  /**
   * Provider slug used for OpenRouter `provider.order` routing. Resolved from
   * the matched provider metadata, falling back to a normalized form of the
   * endpoint's provider name when slug drift breaks the join.
   */
  providerSlug: string;
  /** Endpoint tag (the routing-specific suffix, e.g. `cerebras/fp16`). */
  tag: string | null;
  /** Model id this endpoint serves (when the endpoint reports it). */
  modelId: string | null;
  /** Weight quantization (e.g. `fp16`, `fp8`); `null` when unspecified. */
  quantization: string | null;
  /** Context length in tokens; `null` when the endpoint omits it. */
  contextLength: number | null;
  /** Median (p50) inter-token latency in seconds; `null` when unknown. */
  latencyP50: number | null;
  /** Median (p50) throughput in tokens/sec; `null` when unknown. */
  throughputP50: number | null;
  /** Prompt price per token as a number; `null` when unparseable/absent. */
  promptPrice: number | null;
  /** Completion price per token as a number; `null` when unparseable/absent. */
  completionPrice: number | null;
  /** `true` when both prompt and completion price to exactly zero (free). */
  isFree: boolean;
  /** 30-minute uptime fraction in [0, 1]; `null` when unknown. */
  uptime30m: number | null;
  /** Supported request parameters (e.g. `tools`, `response_format`). */
  supportedParameters: string[];
  /** Normalized data/privacy policy fields. */
  privacy: AcceleratorPrivacyInfo;
}

/** The full non-secret accelerator catalog view model. */
export interface AcceleratorCatalogViewModel {
  /** Model id the endpoints belong to, when the catalog reports it. */
  modelId: string | null;
  /** Normalized endpoint rows, in catalog order. */
  endpoints: AcceleratorEndpointView[];
}

/** Ranking/filtering preset for accelerator discovery. */
export type AcceleratorPreset =
  | "low_latency"
  | "high_throughput"
  | "privacy_zdr";

function trimmedOrNull(value: string | null | undefined): string | null {
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
}

/**
 * Parse a price string OpenRouter returns (scientific-notation float as a
 * string, e.g. `"0.000003"`). Returns `null` when absent or unparseable so a
 * malformed price never poisons the sort.
 */
function parsePrice(value: string | null | undefined): number | null {
  if (value == null) return null;
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : null;
}

function p50(
  stats: OpenRouterPercentileStats | null | undefined,
): number | null {
  const value = stats?.p50;
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/**
 * Normalize a provider name into a routing slug as a fallback when the provider
 * metadata join fails (slug drift, or a provider absent from `/providers`).
 * OpenRouter slugs are lowercase, space→hyphen — close enough for routing and
 * never worse than the previous hardcoded guess.
 */
function nameToSlugFallback(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/\s+/g, "-")
    .replace(/[^a-z0-9-]/g, "");
}

/**
 * Index providers by every key an endpoint might join on: exact slug, exact
 * name, and case-folded variants of both. This survives slug drift (the
 * endpoint reports a name the `/providers` slug no longer matches) by also
 * matching on name.
 */
function indexProviders(
  providers: OpenRouterProvider[],
): Map<string, OpenRouterProvider> {
  const index = new Map<string, OpenRouterProvider>();
  const put = (
    key: string | null | undefined,
    provider: OpenRouterProvider,
  ) => {
    const normalized = trimmedOrNull(key)?.toLowerCase();
    if (normalized && !index.has(normalized)) index.set(normalized, provider);
  };
  for (const provider of providers) {
    put(provider.slug, provider);
    put(provider.name, provider);
    if (provider.name) put(nameToSlugFallback(provider.name), provider);
  }
  return index;
}

function resolveProvider(
  endpoint: OpenRouterEndpoint,
  index: Map<string, OpenRouterProvider>,
): OpenRouterProvider | null {
  const candidates = [
    endpoint.provider_name,
    endpoint.tag,
    // The tag is often `provider/quant`; the leading segment is the slug-ish.
    endpoint.tag?.split("/")[0],
  ];
  for (const candidate of candidates) {
    const key = trimmedOrNull(candidate)?.toLowerCase();
    if (!key) continue;
    const match = index.get(key);
    if (match) return match;
  }
  return null;
}

function normalizePrivacy(
  provider: OpenRouterProvider | null,
): AcceleratorPrivacyInfo {
  return {
    privacyPolicyUrl: trimmedOrNull(provider?.privacy_policy_url),
    termsOfServiceUrl: trimmedOrNull(provider?.terms_of_service_url),
    statusPageUrl: trimmedOrNull(provider?.status_page_url),
    headquarters: trimmedOrNull(provider?.headquarters),
    datacenters: (provider?.datacenters ?? []).filter((dc): dc is string =>
      Boolean(dc?.trim()),
    ),
    // Not exposed per-endpoint by the public catalog → unknown, never guessed.
    zeroDataRetention: null,
  };
}

function normalizeEndpoint(
  endpoint: OpenRouterEndpoint,
  index: Map<string, OpenRouterProvider>,
): AcceleratorEndpointView {
  const provider = resolveProvider(endpoint, index);
  const providerName =
    trimmedOrNull(provider?.name) ??
    trimmedOrNull(endpoint.provider_name) ??
    trimmedOrNull(endpoint.tag?.split("/")[0]) ??
    "Unknown provider";
  const providerSlug =
    trimmedOrNull(provider?.slug) ?? nameToSlugFallback(providerName);

  const promptPrice = parsePrice(endpoint.pricing?.prompt);
  const completionPrice = parsePrice(endpoint.pricing?.completion);

  return {
    providerName,
    providerSlug,
    tag: trimmedOrNull(endpoint.tag),
    modelId: trimmedOrNull(endpoint.model_id),
    quantization: trimmedOrNull(endpoint.quantization),
    contextLength:
      typeof endpoint.context_length === "number"
        ? endpoint.context_length
        : null,
    latencyP50: p50(endpoint.latency_last_30m),
    throughputP50: p50(endpoint.throughput_last_30m),
    promptPrice,
    completionPrice,
    isFree: promptPrice === 0 && completionPrice === 0,
    uptime30m:
      typeof endpoint.uptime_last_30m === "number"
        ? endpoint.uptime_last_30m
        : null,
    supportedParameters: (endpoint.supported_parameters ?? []).filter(
      (p): p is string => Boolean(p?.trim()),
    ),
    privacy: normalizePrivacy(provider),
  };
}

/**
 * Build the non-secret accelerator catalog view model from the saved-key
 * endpoint + provider catalog payloads. Tolerates a `null`/empty endpoints
 * payload (returns an empty view model) and missing provider metadata.
 */
export function buildAcceleratorCatalog(
  endpoints: OpenRouterModelEndpoints | null | undefined,
  providers: OpenRouterProvider[] | null | undefined,
): AcceleratorCatalogViewModel {
  const providerIndex = indexProviders(providers ?? []);
  const rows = (endpoints?.endpoints ?? []).map((endpoint) =>
    normalizeEndpoint(endpoint, providerIndex),
  );
  return {
    modelId: trimmedOrNull(endpoints?.id),
    endpoints: rows,
  };
}

// Sort comparators push `null` metrics to the end so endpoints that report the
// ranked metric always rank ahead of those that don't (a missing latency must
// not masquerade as the fastest).
function ascendingNullsLast(a: number | null, b: number | null): number {
  if (a == null && b == null) return 0;
  if (a == null) return 1;
  if (b == null) return -1;
  return a - b;
}

function descendingNullsLast(a: number | null, b: number | null): number {
  if (a == null && b == null) return 0;
  if (a == null) return 1;
  if (b == null) return -1;
  return b - a;
}

/**
 * Whether an endpoint carries enough verifiable privacy/ToS provenance to be a
 * candidate for the privacy preset. We require at least a privacy policy OR ToS
 * URL — never inferring ZDR from absence.
 */
function hasPrivacyProvenance(view: AcceleratorEndpointView): boolean {
  return (
    view.privacy.privacyPolicyUrl != null ||
    view.privacy.termsOfServiceUrl != null
  );
}

/**
 * Rank/filter the accelerator endpoints for a discovery preset:
 *   - `low_latency`     → endpoints with a known latency, fastest p50 first.
 *   - `high_throughput` → endpoints with a known throughput, highest p50 first
 *     (the "Nitro" intent: maximize tokens/sec).
 *   - `privacy_zdr`     → only endpoints with verifiable privacy/ToS provenance
 *     (or a known-true ZDR flag), ordered by lowest latency among them.
 *
 * Always returns a NEW array; never mutates the input. Endpoints that lack the
 * ranked metric are kept (sorted last) for `low_latency`/`high_throughput` so
 * the view still lists them, but `privacy_zdr` filters out unverifiable ones.
 */
export function rankAccelerators(
  endpoints: AcceleratorEndpointView[],
  preset: AcceleratorPreset,
): AcceleratorEndpointView[] {
  const rows = [...endpoints];
  switch (preset) {
    case "low_latency":
      return rows.sort((a, b) =>
        ascendingNullsLast(a.latencyP50, b.latencyP50),
      );
    case "high_throughput":
      return rows.sort((a, b) =>
        descendingNullsLast(a.throughputP50, b.throughputP50),
      );
    case "privacy_zdr":
      return rows
        .filter(
          (view) =>
            view.privacy.zeroDataRetention === true ||
            hasPrivacyProvenance(view),
        )
        .sort((a, b) => ascendingNullsLast(a.latencyP50, b.latencyP50));
    default:
      return rows;
  }
}

/**
 * Derive an OpenRouter `provider.order` slug list from a ranked accelerator
 * view. Deduplicates while preserving rank order and drops empty slugs. This is
 * the dynamic replacement for the hardcoded `["cerebras", "groq"]` default —
 * Settings can feed the result straight into the routing policy's `order`.
 */
export function acceleratorProviderOrder(
  endpoints: AcceleratorEndpointView[],
): string[] {
  const order: string[] = [];
  const seen = new Set<string>();
  for (const view of endpoints) {
    const slug = view.providerSlug.trim();
    if (slug && !seen.has(slug)) {
      seen.add(slug);
      order.push(slug);
    }
  }
  return order;
}
