import {
  CREDENTIAL_CONTRACT,
  type CredentialError,
  type CredentialErrorCode,
  type CredentialSafeRecoveryAction,
  type CredentialSetId,
} from "../generated/credentialContract";
import {
  RUNTIME_DIAGNOSTIC_CONTRACT,
  type RuntimeDiagnostic,
  type RuntimeDiagnosticAppError,
  type RuntimeDiagnosticContext,
  type RuntimeErrorCode,
  type RuntimeOperation,
  type RuntimeRetryDelayBucket,
  type RuntimeSafeRecoveryAction,
  type RuntimeStatusClass,
  type RuntimeTransport,
} from "../generated/runtimeDiagnostic";

type UnknownRecord = Record<string, unknown>;

const CANONICAL_CUSTOM_CREDENTIAL_SET_ID = new RegExp(
  CREDENTIAL_CONTRACT.custom_set_policy.canonical_pattern,
);

const SAFE_INTERNAL_CONTEXT = Object.freeze({
  operation: "provider_request" as const,
});

/**
 * Fixed fallback for malformed or unrecognized diagnostic payloads.
 * It intentionally retains nothing from the rejected value.
 */
export const SAFE_INTERNAL_RUNTIME_DIAGNOSTIC: RuntimeDiagnostic =
  Object.freeze({
    kind: "runtime" as const,
    detail: Object.freeze({
      code: "internal" as const,
      retryable: false,
      recovery_action: "none" as const,
      context: SAFE_INTERNAL_CONTEXT,
    }),
  });

function asRecord(value: unknown): UnknownRecord | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as UnknownRecord)
    : null;
}

function isClosedValue<T extends string>(
  vocabulary: readonly T[],
  value: unknown,
): value is T {
  return (
    typeof value === "string" &&
    (vocabulary as readonly string[]).includes(value)
  );
}

function parseCredentialSetId(value: unknown): CredentialSetId | null {
  if (typeof value !== "string") return null;
  if (
    (CREDENTIAL_CONTRACT.built_in_set_ids as readonly string[]).includes(value)
  ) {
    return value as CredentialSetId;
  }
  return CANONICAL_CUSTOM_CREDENTIAL_SET_ID.test(value)
    ? (value as CredentialSetId)
    : null;
}

function parseCredentialDiagnostic(value: unknown): CredentialError | null {
  const detail = asRecord(value);
  if (!detail) return null;
  if (
    !isClosedValue(CREDENTIAL_CONTRACT.vocabulary.error_codes, detail.code) ||
    typeof detail.retryable !== "boolean" ||
    !isClosedValue(
      CREDENTIAL_CONTRACT.vocabulary.recovery_actions,
      detail.recovery_action,
    )
  ) {
    return null;
  }

  let setId: CredentialSetId | null | undefined;
  if (detail.set_id === null) {
    setId = null;
  } else if (detail.set_id !== undefined) {
    setId = parseCredentialSetId(detail.set_id);
    if (!setId) return null;
  }

  const sanitized: CredentialError = {
    code: detail.code as CredentialErrorCode,
    retryable: detail.retryable,
    recovery_action: detail.recovery_action as CredentialSafeRecoveryAction,
  };
  if (setId !== undefined) sanitized.set_id = setId;
  return sanitized;
}

function parseOptionalClosedValue<T extends string>(
  record: UnknownRecord,
  key: string,
  vocabulary: readonly T[],
): T | null | undefined | false {
  const value = record[key];
  if (value === undefined) return undefined;
  if (value === null) return null;
  return isClosedValue(vocabulary, value) ? value : false;
}

function parseRuntimeContext(value: unknown): RuntimeDiagnosticContext | null {
  const context = asRecord(value);
  if (
    !context ||
    !isClosedValue(RUNTIME_DIAGNOSTIC_CONTRACT.operations, context.operation)
  ) {
    return null;
  }

  const transport = parseOptionalClosedValue(
    context,
    "transport",
    RUNTIME_DIAGNOSTIC_CONTRACT.transports,
  );
  const statusClass = parseOptionalClosedValue(
    context,
    "status_class",
    RUNTIME_DIAGNOSTIC_CONTRACT.status_classes,
  );
  const retryDelay = parseOptionalClosedValue(
    context,
    "retry_delay",
    RUNTIME_DIAGNOSTIC_CONTRACT.retry_delay_buckets,
  );
  if (transport === false || statusClass === false || retryDelay === false) {
    return null;
  }

  const sanitized: RuntimeDiagnosticContext = {
    operation: context.operation as RuntimeOperation,
  };
  if (transport !== undefined) {
    sanitized.transport = transport as RuntimeTransport | null;
  }
  if (statusClass !== undefined) {
    sanitized.status_class = statusClass as RuntimeStatusClass | null;
  }
  if (retryDelay !== undefined) {
    sanitized.retry_delay = retryDelay as RuntimeRetryDelayBucket | null;
  }
  return sanitized;
}

function parseRuntimeErrorDiagnostic(value: unknown): RuntimeDiagnostic | null {
  const detail = asRecord(value);
  if (
    !detail ||
    !isClosedValue(RUNTIME_DIAGNOSTIC_CONTRACT.error_codes, detail.code) ||
    typeof detail.retryable !== "boolean" ||
    !isClosedValue(
      RUNTIME_DIAGNOSTIC_CONTRACT.recovery_actions,
      detail.recovery_action,
    )
  ) {
    return null;
  }
  const context = parseRuntimeContext(detail.context);
  if (!context) return null;

  return {
    kind: "runtime",
    detail: {
      code: detail.code as RuntimeErrorCode,
      retryable: detail.retryable,
      recovery_action: detail.recovery_action as RuntimeSafeRecoveryAction,
      context,
    },
  };
}

/** Parse and reconstruct only the accepted closed diagnostic fields. */
export function parseRuntimeDiagnostic(
  value: unknown,
): RuntimeDiagnostic | null {
  const envelope = asRecord(value);
  if (!envelope) return null;
  if (envelope.kind === "credential") {
    const detail = parseCredentialDiagnostic(envelope.detail);
    return detail ? { kind: "credential", detail } : null;
  }
  if (envelope.kind === "runtime") {
    return parseRuntimeErrorDiagnostic(envelope.detail);
  }
  return null;
}

/** Parse the distinct ec13 AppError wire without accepting a `message` alias. */
export function parseRuntimeDiagnosticAppError(
  value: unknown,
): RuntimeDiagnosticAppError | null {
  const error = asRecord(value);
  if (error?.code !== "runtime_diagnostic") return null;
  const diagnostic = parseRuntimeDiagnostic(error.diagnostic);
  return diagnostic ? { code: "runtime_diagnostic", diagnostic } : null;
}

/**
 * Normalize a direct diagnostic or AppError rejection. Malformed values become
 * the fixed internal tuple; raw input is never returned or stringified.
 */
export function normalizeRuntimeDiagnostic(value: unknown): RuntimeDiagnostic {
  return (
    parseRuntimeDiagnosticAppError(value)?.diagnostic ??
    parseRuntimeDiagnostic(value) ??
    SAFE_INTERNAL_RUNTIME_DIAGNOSTIC
  );
}

export type RuntimeDiagnosticTranslationKey =
  | `runtimeDiagnostics.credential.${CredentialErrorCode}`
  | `runtimeDiagnostics.runtime.${RuntimeErrorCode}`;

export interface RuntimeDiagnosticPresentation {
  diagnostic: RuntimeDiagnostic;
  translationKey: RuntimeDiagnosticTranslationKey;
  retryable: boolean;
  recoveryAction: CredentialSafeRecoveryAction | RuntimeSafeRecoveryAction;
  operation?: RuntimeOperation;
}

/**
 * Produce only a closed localization key and safe recovery metadata. Downstream
 * UI code supplies translated copy; this helper never invents provider text.
 */
export function runtimeDiagnosticPresentation(
  value: unknown,
): RuntimeDiagnosticPresentation {
  const diagnostic = normalizeRuntimeDiagnostic(value);
  if (diagnostic.kind === "credential") {
    return {
      diagnostic,
      translationKey: `runtimeDiagnostics.credential.${diagnostic.detail.code}`,
      retryable: diagnostic.detail.retryable,
      recoveryAction: diagnostic.detail.recovery_action,
    };
  }
  return {
    diagnostic,
    translationKey: `runtimeDiagnostics.runtime.${diagnostic.detail.code}`,
    retryable: diagnostic.detail.retryable,
    recoveryAction: diagnostic.detail.recovery_action,
    operation: diagnostic.detail.context.operation,
  };
}
