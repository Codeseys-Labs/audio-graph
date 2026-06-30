/**
 * Pure controller helpers hoisted out of `SettingsPage.tsx` (blueprint §5,
 * impl-plan Phase 1). This module owns the OpenRouter routing-policy
 * normalization/inference cluster — a cohesive block of pure functions that
 * map between the `OpenRouterRoutingPolicy` wire shape and the UI's preset
 * model. No React, no component closure state: every function here is a pure
 * transform, so it relocates with zero behavior change and is unit-testable in
 * isolation.
 *
 * Only the entry points the shell + LLM panel actually call are `export`ed; the
 * normalization helpers are module-private (used only by each other).
 */

import type { OpenRouterRoutingPolicy } from "../../types";
import type { SettingsState } from "../settingsTypes";

export const DEFAULT_OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1";

export function normalizeOpenRouterBaseUrl(baseUrl: string): string {
  const trimmed = baseUrl.trim().replace(/\/+$/, "");
  return trimmed || DEFAULT_OPENROUTER_BASE_URL;
}

export function openRouterModelsCacheKey(
  baseUrl: string,
  apiKey: string,
): string {
  const authState = apiKey.trim() ? "with-key" : "no-key";
  return `${normalizeOpenRouterBaseUrl(baseUrl)}|${authState}`;
}

export function parseOpenRouterProviderList(value: string): string[] {
  return value
    .split(/[\n,]+/)
    .map((entry) => entry.trim())
    .filter(
      (entry, index, entries) => entry && entries.indexOf(entry) === index,
    );
}

function openRouterProviderListText(values: readonly string[] = []): string {
  return values.join(", ");
}

function openRouterRoutingPolicyBase(
  policy: Partial<OpenRouterRoutingPolicy> = {},
): OpenRouterRoutingPolicy {
  return {
    order: [],
    only: [],
    ignore: [],
    quantizations: [],
    ...policy,
  };
}

const OPENROUTER_ROUTING_POLICY_KEYS = new Set<keyof OpenRouterRoutingPolicy>([
  "order",
  "only",
  "ignore",
  "allow_fallbacks",
  "require_parameters",
  "data_collection",
  "zdr",
  "enforce_distillable_text",
  "quantizations",
  "sort",
  "preferred_min_throughput",
  "preferred_max_latency",
  "max_price",
]);

function openRouterLowLatencyRoutingPolicy(): OpenRouterRoutingPolicy {
  return openRouterRoutingPolicyBase({
    sort: { by: "latency", partition: "model" },
    preferred_max_latency: { p50: 0.75, p90: 2.0 },
  });
}

function openRouterHighThroughputRoutingPolicy(): OpenRouterRoutingPolicy {
  return openRouterRoutingPolicyBase({
    sort: { by: "throughput", partition: "model" },
    preferred_min_throughput: { p50: 40, p90: 20 },
  });
}

function openRouterPrivacyZdrRoutingPolicy(): OpenRouterRoutingPolicy {
  return openRouterRoutingPolicyBase({
    data_collection: "deny",
    zdr: true,
  });
}

const OPENROUTER_LOW_LATENCY_ROUTING_POLICY =
  openRouterLowLatencyRoutingPolicy();
const OPENROUTER_HIGH_THROUGHPUT_ROUTING_POLICY =
  openRouterHighThroughputRoutingPolicy();
const OPENROUTER_PRIVACY_ZDR_ROUTING_POLICY =
  openRouterPrivacyZdrRoutingPolicy();

function openRouterRoutingPolicyHasOnlyKnownKeys(
  policy: OpenRouterRoutingPolicy,
): boolean {
  return Object.keys(policy as unknown as Record<string, unknown>).every(
    (key) =>
      OPENROUTER_ROUTING_POLICY_KEYS.has(key as keyof OpenRouterRoutingPolicy),
  );
}

function normalizeOpenRouterRoutingSort(
  sort: OpenRouterRoutingPolicy["sort"] | undefined,
):
  | OpenRouterRoutingPolicy["sort"]
  | {
      by: string;
      partition?: string;
    }
  | undefined {
  if (sort === undefined || typeof sort === "string") return sort;
  return {
    by: sort.by,
    ...(sort.partition === undefined ? {} : { partition: sort.partition }),
  };
}

function normalizeOpenRouterPerformancePreference(
  value: OpenRouterRoutingPolicy["preferred_max_latency"] | undefined,
):
  | OpenRouterRoutingPolicy["preferred_max_latency"]
  | {
      p50?: number;
      p75?: number;
      p90?: number;
      p99?: number;
    }
  | undefined {
  if (value === undefined || typeof value === "number") return value;
  return {
    ...(value.p50 === undefined ? {} : { p50: value.p50 }),
    ...(value.p75 === undefined ? {} : { p75: value.p75 }),
    ...(value.p90 === undefined ? {} : { p90: value.p90 }),
    ...(value.p99 === undefined ? {} : { p99: value.p99 }),
  };
}

function normalizeOpenRouterMaxPrice(
  value: OpenRouterRoutingPolicy["max_price"] | undefined,
): OpenRouterRoutingPolicy["max_price"] | undefined {
  if (value === undefined) return undefined;
  return {
    ...(value.prompt === undefined ? {} : { prompt: value.prompt }),
    ...(value.completion === undefined ? {} : { completion: value.completion }),
    ...(value.request === undefined ? {} : { request: value.request }),
    ...(value.image === undefined ? {} : { image: value.image }),
  };
}

function normalizeOpenRouterRoutingPolicyForComparison(
  policy: OpenRouterRoutingPolicy,
) {
  return {
    order: [...(policy.order ?? [])],
    only: [...(policy.only ?? [])],
    ignore: [...(policy.ignore ?? [])],
    ...(policy.allow_fallbacks === undefined
      ? {}
      : { allow_fallbacks: policy.allow_fallbacks }),
    ...(policy.require_parameters === undefined
      ? {}
      : { require_parameters: policy.require_parameters }),
    ...(policy.data_collection === undefined
      ? {}
      : { data_collection: policy.data_collection }),
    ...(policy.zdr === undefined ? {} : { zdr: policy.zdr }),
    ...(policy.enforce_distillable_text === undefined
      ? {}
      : { enforce_distillable_text: policy.enforce_distillable_text }),
    quantizations: [...(policy.quantizations ?? [])],
    ...(policy.sort === undefined
      ? {}
      : { sort: normalizeOpenRouterRoutingSort(policy.sort) }),
    ...(policy.preferred_min_throughput === undefined
      ? {}
      : {
          preferred_min_throughput: normalizeOpenRouterPerformancePreference(
            policy.preferred_min_throughput,
          ),
        }),
    ...(policy.preferred_max_latency === undefined
      ? {}
      : {
          preferred_max_latency: normalizeOpenRouterPerformancePreference(
            policy.preferred_max_latency,
          ),
        }),
    ...(policy.max_price === undefined
      ? {}
      : { max_price: normalizeOpenRouterMaxPrice(policy.max_price) }),
  };
}

function isOpenRouterRoutingPolicyExactShape(
  policy: OpenRouterRoutingPolicy,
  expected: OpenRouterRoutingPolicy,
): boolean {
  return (
    openRouterRoutingPolicyHasOnlyKnownKeys(policy) &&
    JSON.stringify(normalizeOpenRouterRoutingPolicyForComparison(policy)) ===
      JSON.stringify(normalizeOpenRouterRoutingPolicyForComparison(expected))
  );
}

function isOpenRouterRoutingPolicyEmpty(
  policy: OpenRouterRoutingPolicy | null | undefined,
): boolean {
  return (
    !policy ||
    isOpenRouterRoutingPolicyExactShape(policy, openRouterRoutingPolicyBase())
  );
}

export function inferOpenRouterRoutingPreset(
  policy: OpenRouterRoutingPolicy | null | undefined,
  legacyProviderOrder: readonly string[] = [],
): SettingsState["openrouterRoutingPreset"] {
  if (!policy || isOpenRouterRoutingPolicyEmpty(policy)) {
    return legacyProviderOrder.length > 0 ? "legacy" : "balanced";
  }
  const order = policy.order ?? [];
  const only = policy.only ?? [];
  if (
    order.length > 0 &&
    only.length === order.length &&
    only.every((provider, index) => provider === order[index]) &&
    isOpenRouterRoutingPolicyExactShape(
      policy,
      openRouterRoutingPolicyBase({
        order,
        only: order,
        allow_fallbacks: false,
      }),
    )
  ) {
    return "strict_accelerator";
  }
  if (
    isOpenRouterRoutingPolicyExactShape(
      policy,
      OPENROUTER_PRIVACY_ZDR_ROUTING_POLICY,
    )
  ) {
    return "privacy_zdr";
  }
  if (
    isOpenRouterRoutingPolicyExactShape(
      policy,
      OPENROUTER_LOW_LATENCY_ROUTING_POLICY,
    )
  ) {
    return "low_latency";
  }
  if (
    isOpenRouterRoutingPolicyExactShape(
      policy,
      OPENROUTER_HIGH_THROUGHPUT_ROUTING_POLICY,
    )
  ) {
    return "high_throughput";
  }
  return "custom";
}

export function openRouterProviderOrderTextForSettings(
  policy: OpenRouterRoutingPolicy | null | undefined,
  legacyProviderOrder: readonly string[] = [],
): string {
  const order = policy?.order ?? [];
  if (order.length > 0) return openRouterProviderListText(order);
  const only = policy?.only ?? [];
  if (only.length > 0) return openRouterProviderListText(only);
  return openRouterProviderListText(legacyProviderOrder);
}

export function buildOpenRouterRoutingPolicy(
  preset: SettingsState["openrouterRoutingPreset"],
  providerOrderText: string,
  existingPolicy: OpenRouterRoutingPolicy | null,
): OpenRouterRoutingPolicy | null {
  switch (preset) {
    case "legacy":
    case "balanced":
      return null;
    case "custom":
      return existingPolicy;
    case "low_latency":
      return openRouterLowLatencyRoutingPolicy();
    case "high_throughput":
      return openRouterHighThroughputRoutingPolicy();
    case "privacy_zdr":
      return openRouterPrivacyZdrRoutingPolicy();
    case "strict_accelerator": {
      const providers = parseOpenRouterProviderList(providerOrderText);
      return openRouterRoutingPolicyBase({
        order: providers,
        only: providers,
        allow_fallbacks: false,
      });
    }
  }
}
