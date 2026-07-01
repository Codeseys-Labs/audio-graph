import { describe, expect, it } from "vitest";
import type {
  OpenRouterEndpoint,
  OpenRouterModelEndpoints,
  OpenRouterProvider,
} from "../types";
import {
  acceleratorProviderOrder,
  buildAcceleratorCatalog,
  rankAccelerators,
} from "./openrouterCatalog";

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
    supported_parameters: ["tools", "response_format"],
    uptime_last_30m: 0.999,
    uptime_last_5m: null,
    uptime_last_1d: null,
    supports_implicit_caching: null,
    latency_last_30m: { p50: 0.15, p75: null, p90: null, p99: null },
    throughput_last_30m: { p50: 2100, p75: null, p90: null, p99: null },
    status: null,
    ...overrides,
  };
}

function modelEndpoints(
  endpoints: OpenRouterEndpoint[],
  overrides: Partial<OpenRouterModelEndpoints> = {},
): OpenRouterModelEndpoints {
  return {
    id: "meta-llama/llama-3.1-70b",
    name: "Llama 3.1 70B",
    created: null,
    description: null,
    architecture: null,
    endpoints,
    ...overrides,
  };
}

describe("buildAcceleratorCatalog: dynamic discovery", () => {
  it("normalizes accelerator endpoints from the saved-key catalog payloads", () => {
    const vm = buildAcceleratorCatalog(
      modelEndpoints([
        endpoint({ provider_name: "Cerebras", tag: "cerebras/fp16" }),
        endpoint({
          provider_name: "Groq",
          tag: "groq/fp8",
          quantization: "fp8",
          latency_last_30m: { p50: 0.25 },
          throughput_last_30m: { p50: 1200 },
          pricing: { prompt: "0.00000059", completion: "0.00000079" },
        }),
      ]),
      [provider("cerebras", "Cerebras"), provider("groq", "Groq")],
    );

    expect(vm.modelId).toBe("meta-llama/llama-3.1-70b");
    expect(vm.endpoints).toHaveLength(2);
    const [cerebras, groq] = vm.endpoints;
    expect(cerebras.providerName).toBe("Cerebras");
    expect(cerebras.providerSlug).toBe("cerebras");
    expect(cerebras.quantization).toBe("fp16");
    expect(cerebras.contextLength).toBe(131072);
    expect(cerebras.latencyP50).toBe(0.15);
    expect(cerebras.throughputP50).toBe(2100);
    expect(cerebras.promptPrice).toBeCloseTo(0.0000006);
    expect(cerebras.isFree).toBe(false);
    expect(cerebras.supportedParameters).toEqual(["tools", "response_format"]);
    expect(cerebras.privacy.privacyPolicyUrl).toBe(
      "https://example.com/cerebras/privacy",
    );
    expect(groq.providerSlug).toBe("groq");
    expect(groq.quantization).toBe("fp8");
  });

  it("does NOT rely on a hardcoded provider list — slug drift still joins by name", () => {
    // Endpoint reports provider_name "SambaNova" but the /providers slug for it
    // drifted to "sambanova-cloud". Joining on name (case-folded) still resolves
    // the policy URLs; the slug comes from the matched provider, not a guess.
    const vm = buildAcceleratorCatalog(
      modelEndpoints([
        endpoint({
          provider_name: "SambaNova",
          tag: "SambaNova/bf16",
          quantization: "bf16",
        }),
      ]),
      [
        provider("sambanova-cloud", "SambaNova", {
          privacy_policy_url: "https://sambanova.ai/privacy",
        }),
      ],
    );

    const [row] = vm.endpoints;
    expect(row.providerName).toBe("SambaNova");
    expect(row.providerSlug).toBe("sambanova-cloud");
    expect(row.privacy.privacyPolicyUrl).toBe("https://sambanova.ai/privacy");
  });

  it("falls back to a normalized slug when the provider is absent from /providers", () => {
    // A brand-new accelerator not yet in the providers catalog must not break
    // the view — derive a routing slug from the name and mark policy unknown.
    const vm = buildAcceleratorCatalog(
      modelEndpoints([
        endpoint({ provider_name: "New Accel Inc", tag: "new-accel/fp16" }),
      ]),
      [],
    );

    const [row] = vm.endpoints;
    expect(row.providerName).toBe("New Accel Inc");
    expect(row.providerSlug).toBe("new-accel-inc");
    // Unverifiable policy fields are null, never fabricated.
    expect(row.privacy.privacyPolicyUrl).toBeNull();
    expect(row.privacy.termsOfServiceUrl).toBeNull();
    expect(row.privacy.zeroDataRetention).toBeNull();
  });
});

describe("buildAcceleratorCatalog: missing metadata, empty/error states", () => {
  it("returns an empty view model for a null endpoints payload (error/uninit state)", () => {
    const vm = buildAcceleratorCatalog(null, [
      provider("cerebras", "Cerebras"),
    ]);
    expect(vm.modelId).toBeNull();
    expect(vm.endpoints).toEqual([]);
  });

  it("returns an empty view model for an empty endpoints array", () => {
    const vm = buildAcceleratorCatalog(modelEndpoints([]), null);
    expect(vm.endpoints).toEqual([]);
  });

  it("tolerates an endpoint missing every optional field without throwing", () => {
    const vm = buildAcceleratorCatalog(
      modelEndpoints([
        {
          provider_name: null,
          tag: null,
          quantization: null,
          context_length: null,
          pricing: null,
          latency_last_30m: null,
          throughput_last_30m: null,
          uptime_last_30m: null,
          supported_parameters: [],
        },
      ]),
      null,
    );

    const [row] = vm.endpoints;
    expect(row.providerName).toBe("Unknown provider");
    expect(row.providerSlug).toBe("unknown-provider");
    expect(row.quantization).toBeNull();
    expect(row.contextLength).toBeNull();
    expect(row.latencyP50).toBeNull();
    expect(row.throughputP50).toBeNull();
    expect(row.promptPrice).toBeNull();
    expect(row.completionPrice).toBeNull();
    expect(row.isFree).toBe(false);
    expect(row.supportedParameters).toEqual([]);
  });

  it("treats unparseable pricing as null (does not poison isFree)", () => {
    const vm = buildAcceleratorCatalog(
      modelEndpoints([
        endpoint({ pricing: { prompt: "not-a-number", completion: "" } }),
      ]),
      null,
    );
    const [row] = vm.endpoints;
    expect(row.promptPrice).toBeNull();
    expect(row.completionPrice).toBeNull();
    expect(row.isFree).toBe(false);
  });

  it("flags a free endpoint when both prices are exactly zero", () => {
    const vm = buildAcceleratorCatalog(
      modelEndpoints([endpoint({ pricing: { prompt: "0", completion: "0" } })]),
      null,
    );
    expect(vm.endpoints[0].isFree).toBe(true);
  });
});

describe("rankAccelerators presets", () => {
  const sample = buildAcceleratorCatalog(
    modelEndpoints([
      endpoint({
        provider_name: "Cerebras",
        tag: "cerebras/fp16",
        latency_last_30m: { p50: 0.15 },
        throughput_last_30m: { p50: 2100 },
      }),
      endpoint({
        provider_name: "Groq",
        tag: "groq/fp8",
        latency_last_30m: { p50: 0.25 },
        throughput_last_30m: { p50: 1200 },
      }),
      endpoint({
        provider_name: "Mystery",
        tag: "mystery/unknown",
        latency_last_30m: null,
        throughput_last_30m: null,
      }),
    ]),
    [
      provider("cerebras", "Cerebras"),
      provider("groq", "Groq"),
      // Mystery has no policy URLs → unverifiable for the privacy preset.
      provider("mystery", "Mystery", {
        privacy_policy_url: null,
        terms_of_service_url: null,
      }),
    ],
  );

  it("low_latency orders by fastest p50 first, unknown latency last", () => {
    const ranked = rankAccelerators(sample.endpoints, "low_latency");
    expect(ranked.map((e) => e.providerName)).toEqual([
      "Cerebras",
      "Groq",
      "Mystery",
    ]);
  });

  it("high_throughput (Nitro intent) orders by highest p50 first", () => {
    const ranked = rankAccelerators(sample.endpoints, "high_throughput");
    expect(ranked.map((e) => e.providerName)).toEqual([
      "Cerebras",
      "Groq",
      "Mystery",
    ]);
  });

  it("privacy_zdr drops endpoints without verifiable privacy provenance", () => {
    const ranked = rankAccelerators(sample.endpoints, "privacy_zdr");
    // Mystery has no privacy/ToS URL and ZDR is unknown → filtered out.
    expect(ranked.map((e) => e.providerName)).toEqual(["Cerebras", "Groq"]);
  });

  it("does not mutate the input array", () => {
    const before = sample.endpoints.map((e) => e.providerName);
    rankAccelerators(sample.endpoints, "low_latency");
    expect(sample.endpoints.map((e) => e.providerName)).toEqual(before);
  });
});

describe("acceleratorProviderOrder", () => {
  it("derives a deduped slug order from ranked endpoints (replaces the hardcoded default)", () => {
    const vm = buildAcceleratorCatalog(
      modelEndpoints([
        endpoint({ provider_name: "Cerebras", tag: "cerebras/fp16" }),
        endpoint({
          provider_name: "Groq",
          tag: "groq/fp8",
          latency_last_30m: { p50: 0.25 },
        }),
        // Duplicate provider (different quant) must not double the slug.
        endpoint({
          provider_name: "Cerebras",
          tag: "cerebras/fp8",
          quantization: "fp8",
          latency_last_30m: { p50: 0.1 },
        }),
      ]),
      [provider("cerebras", "Cerebras"), provider("groq", "Groq")],
    );

    const ranked = rankAccelerators(vm.endpoints, "low_latency");
    expect(acceleratorProviderOrder(ranked)).toEqual(["cerebras", "groq"]);
  });

  it("returns an empty order for an empty catalog", () => {
    expect(acceleratorProviderOrder([])).toEqual([]);
  });
});
