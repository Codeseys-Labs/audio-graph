import { GENERATED_PROVIDER_REGISTRY } from "../generated/providerRegistry";
import type {
  CredentialPresence,
  ProviderDescriptor,
  ProviderModelCatalogItem,
  ProviderReadiness,
  ProviderStage,
  ProviderStatus,
} from "../types";

export interface ProviderSettingsOption<T extends string> {
  value: T;
  label: string;
  descriptor: ProviderDescriptor;
}

export const PROVIDER_DESCRIPTORS = new Map(
  GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
) as ReadonlyMap<string, ProviderDescriptor>;

export function providerDescriptorForSettingsVariant<T extends string>(
  stage: ProviderStage,
  settingsVariant: T,
): ProviderDescriptor | null {
  return (
    GENERATED_PROVIDER_REGISTRY.find(
      (provider) =>
        provider.stage === stage &&
        provider.settings_variant === settingsVariant,
    ) ?? null
  );
}

export function providerIdForSettingsVariant<T extends string>(
  stage: ProviderStage,
  settingsVariant: T,
): string {
  return (
    providerDescriptorForSettingsVariant(stage, settingsVariant)?.id ??
    `${stage}.${settingsVariant}`
  );
}

export function defaultModelForProvider(providerId: string): string {
  return PROVIDER_DESCRIPTORS.get(providerId)?.default_model ?? "";
}

export function implementedProviderOptionsForStage<T extends string>(
  stage: ProviderStage,
  settingsVariants: readonly T[],
): ProviderSettingsOption<T>[] {
  return settingsVariants.flatMap((settingsVariant) => {
    const descriptor = providerDescriptorForSettingsVariant(
      stage,
      settingsVariant,
    );
    if (descriptor?.status !== "implemented") return [];

    return [
      {
        value: settingsVariant,
        label: descriptor.display_name,
        descriptor,
      },
    ];
  });
}

function formatProviderCredentialKeys(keys: readonly string[]): string {
  if (keys.length <= 2) return keys.join(", ");
  return `${keys.slice(0, 2).join(", ")} +${keys.length - 2} more`;
}

export function providerStatusLabel(status: ProviderStatus): string {
  switch (status) {
    case "implemented":
      return "Implemented";
    case "planned":
      return "Planned";
    case "watch":
      return "Watch candidate";
    case "enterprise_watch":
      return "Enterprise watch";
    case "rejected":
      return "Rejected";
  }
}

export function providerNotSelectableLabel(
  descriptor: ProviderDescriptor,
): string {
  switch (descriptor.status) {
    case "implemented":
      return "This provider is not selectable from Settings yet.";
    case "planned":
      return "Planned providers are not selectable.";
    case "watch":
      return "Watch candidates are not selectable from Settings.";
    case "enterprise_watch":
      return "Enterprise watch candidates are not selectable from Settings.";
    case "rejected":
      return "Rejected providers are not selectable.";
  }
}

export function providerRoadmapAuthLabel(
  descriptor: ProviderDescriptor,
): string | null {
  switch (descriptor.roadmap?.auth_schema) {
    case "not_required":
      return "No auth required";
    case "wired":
      return "Credential schema wired";
    case "required_not_wired":
      return "Auth required; credential schema not wired";
    case "flexible_external":
      return "External or flexible auth";
    case undefined:
      return null;
  }
}

export function providerCredentialKeysLabel(
  descriptor: ProviderDescriptor,
): string {
  if (descriptor.credential_keys.length > 0) {
    return formatProviderCredentialKeys(descriptor.credential_keys);
  }

  switch (descriptor.roadmap?.auth_schema) {
    case "required_not_wired":
      return "Credential schema not wired";
    case "flexible_external":
      return "External credential flow";
    default:
      return "None";
  }
}

export type ProviderCredentialPresenceLookup = Partial<
  Record<string, Pick<CredentialPresence, "present">>
>;

export function providerCapabilityCredentialLabel(
  descriptor: ProviderDescriptor,
  credentialPresence: ProviderCredentialPresenceLookup,
): string {
  if (descriptor.credential_keys.length === 0) {
    switch (descriptor.roadmap?.auth_schema) {
      case "required_not_wired":
        return "Auth required; credential schema not wired";
      case "flexible_external":
        return "External credential flow";
      default:
        return descriptor.lifecycle.auth === "none"
          ? "No credential required"
          : "Credential schema not declared";
    }
  }

  const savedKeys = descriptor.credential_keys.filter(
    (key) => credentialPresence[key]?.present === true,
  );
  if (savedKeys.length > 0) {
    return `Saved: ${formatProviderCredentialKeys(savedKeys)}`;
  }

  return `Needs: ${formatProviderCredentialKeys(descriptor.credential_keys)}`;
}

export function generatedModelCatalogForProvider(
  providerId: string,
): ProviderModelCatalogItem[] {
  const descriptor = PROVIDER_DESCRIPTORS.get(providerId);
  if (!descriptor) return [];

  if (descriptor.fixed_model_catalog?.length) {
    return descriptor.fixed_model_catalog;
  }

  const items = descriptor.local_models.map((model) => ({
    id: model.model_id,
    display_name: model.model_id,
    is_default: model.model_id === descriptor.default_model,
  }));

  if (
    descriptor.default_model &&
    !items.some((item) => item.id === descriptor.default_model)
  ) {
    items.push({
      id: descriptor.default_model,
      display_name: descriptor.default_model,
      is_default: true,
    });
  }

  return items;
}

export function modelCatalogForProvider(
  readiness: Record<string, ProviderReadiness>,
  providerId: string,
): ProviderModelCatalogItem[] {
  const providerReadiness = readiness[providerId];
  const backendCatalog =
    providerId === "tts.deepgram_aura"
      ? (providerReadiness?.voice_catalog ??
        providerReadiness?.model_catalog ??
        [])
      : (providerReadiness?.model_catalog ?? []);
  return backendCatalog.length
    ? backendCatalog
    : generatedModelCatalogForProvider(providerId);
}
