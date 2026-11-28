import type {
  CredentialErrorCode,
  CredentialSafeRecoveryAction,
} from "../generated/credentialContract";
import type {
  RuntimeDiagnostic,
  RuntimeErrorCode,
  RuntimeSafeRecoveryAction,
} from "../generated/runtimeDiagnostic";
import type { ProviderReadiness, ProviderReadinessNote } from "../types";

/** A provider failure as the backend types it: the `runtime/<code>` arm. */
export function runtimeFailure(
  code: RuntimeErrorCode,
  recoveryAction: RuntimeSafeRecoveryAction = "none",
  retryable = false,
): RuntimeDiagnostic {
  return {
    kind: "runtime",
    detail: {
      code,
      retryable,
      recovery_action: recoveryAction,
      context: { operation: "provider_readiness", transport: "http" },
    },
  };
}

/** A credential-store failure as the backend types it: the `credential/<code>` arm. */
export function credentialStoreFailure(
  code: CredentialErrorCode,
  recoveryAction: CredentialSafeRecoveryAction,
  retryable = false,
): RuntimeDiagnostic {
  return {
    kind: "credential",
    detail: { code, retryable, recovery_action: recoveryAction },
  };
}

/** The `status` + `note` + `diagnostic` triple of a failed readiness entry. */
export function failedReadiness(
  diagnostic: RuntimeDiagnostic,
): Pick<ProviderReadiness, "status" | "note" | "diagnostic"> {
  return { status: "error", note: { kind: "failed" }, diagnostic };
}

/**
 * Every readiness note kind with the copy each locale renders for it. The
 * surfaces that show readiness all assert against this one table.
 */
export const READINESS_NOTE_CASES: ReadonlyArray<{
  note: ProviderReadinessNote;
  en: string;
  pt: string;
}> = [
  {
    note: { kind: "ready_to_check" },
    en: "Ready to check with saved credentials.",
    pt: "Pronto para verificar com as credenciais salvas.",
  },
  {
    note: { kind: "missing_credentials", keys: ["deepgram_api_key"] },
    en: "Missing saved credential: deepgram_api_key.",
    pt: "Credencial salva ausente: deepgram_api_key.",
  },
  {
    note: {
      kind: "missing_credentials",
      keys: ["aws_access_key", "aws_secret_key"],
    },
    en: "Missing saved credentials: aws_access_key, aws_secret_key.",
    pt: "Credenciais salvas ausentes: aws_access_key, aws_secret_key.",
  },
  {
    note: { kind: "aws_profile_required" },
    en: "Enter an AWS profile name to use profile credentials.",
    pt: "Informe um nome de perfil AWS para usar credenciais de perfil.",
  },
  {
    note: { kind: "vertex_project_required" },
    en: "Vertex AI needs a project ID and a location.",
    pt: "O Vertex AI precisa de um ID de projeto e de uma localização.",
  },
  {
    note: { kind: "vertex_not_probed" },
    en: "Vertex AI readiness is not probed automatically yet.",
    pt: "A prontidão do Vertex AI ainda não é verificada automaticamente.",
  },
  {
    note: { kind: "no_automatic_probe" },
    en: "No automatic health probe is available for this provider yet.",
    pt: "Ainda não há verificação automática de integridade para este provedor.",
  },
  {
    note: { kind: "local_models_managed" },
    en: "Local model readiness is checked by the model manager.",
    pt: "A prontidão dos modelos locais é verificada pelo gerenciador de modelos.",
  },
  {
    note: {
      kind: "local_models",
      ready: 1,
      total: 3,
      missing: ["moonshine-tiny-en"],
      invalid: ["moonshine-base-en"],
      planned: true,
    },
    en: "Local model files ready: 1/3 model option(s). Missing: moonshine-tiny-en. Invalid: moonshine-base-en. Provider runtime remains planned and is not selectable yet.",
    pt: "Arquivos de modelo local prontos: 1/3 opção(ões) de modelo. Ausentes: moonshine-tiny-en. Inválidos: moonshine-base-en. O runtime do provedor continua planejado e ainda não é selecionável.",
  },
  {
    note: {
      kind: "local_models",
      ready: 0,
      total: 2,
      missing: [],
      invalid: [],
      planned: false,
    },
    en: "No local model files are ready yet. Download one of 2 model option(s).",
    pt: "Nenhum arquivo de modelo local está pronto ainda. Baixe uma das 2 opção(ões) de modelo.",
  },
  {
    note: { kind: "not_selected" },
    en: "Checked, but this provider is not the one selected.",
    pt: "Verificado, mas este provedor não é o selecionado.",
  },
  {
    note: { kind: "cancelled" },
    en: "The readiness check was cancelled.",
    pt: "A verificação de prontidão foi cancelada.",
  },
  {
    note: { kind: "check_in_progress" },
    en: "A readiness check for this provider is already running.",
    pt: "Uma verificação de prontidão deste provedor já está em andamento.",
  },
  {
    note: { kind: "check_rate_limited", retry_after_secs: 1 },
    en: "A readiness check ran moments ago. Try again in 1 second.",
    pt: "Uma verificação de prontidão foi executada há pouco. Tente novamente em 1 segundo.",
  },
  {
    note: { kind: "check_rate_limited", retry_after_secs: 30 },
    en: "A readiness check ran moments ago. Try again in 30 seconds.",
    pt: "Uma verificação de prontidão foi executada há pouco. Tente novamente em 30 segundos.",
  },
  {
    note: { kind: "verified" },
    en: "Provider check passed.",
    pt: "A verificação do provedor foi aprovada.",
  },
  {
    note: { kind: "failed" },
    en: "The provider check failed.",
    pt: "A verificação do provedor falhou.",
  },
];

/**
 * Typed failures with the sentence each locale renders for the cause and the
 * next step it gets. `recovery` is null when the next step IS the sentence
 * (so it is shown once).
 */
export const FAILURE_CASES: ReadonlyArray<{
  name: string;
  diagnostic: RuntimeDiagnostic;
  message: { en: string; pt: string };
  recovery: { en: string; pt: string } | null;
}> = [
  {
    name: "runtime/credential_rejected",
    diagnostic: runtimeFailure("credential_rejected", "reenter_credential"),
    message: {
      en: "The provider rejected the credential. Re-enter it in Settings.",
      pt: "O provedor rejeitou a credencial. Insira-a novamente em Configurações.",
    },
    recovery: {
      en: "The saved credential was rejected — replace it in this provider section, then run checks.",
      pt: "A credencial salva foi rejeitada — substitua-a nesta seção do provedor e execute as verificações.",
    },
  },
  {
    name: "runtime/credential_expired",
    diagnostic: runtimeFailure("credential_expired", "refresh_credential"),
    message: {
      en: "The provider credential expired. Refresh it before trying again.",
      pt: "A credencial do provedor expirou. Renove-a antes de tentar novamente.",
    },
    recovery: null,
  },
  {
    name: "runtime/permission_denied",
    diagnostic: runtimeFailure("permission_denied", "review_configuration"),
    message: {
      en: "The provider denied access to this operation. Check your account permissions or plan.",
      pt: "O provedor negou o acesso a esta operação. Verifique as permissões ou o plano da sua conta.",
    },
    recovery: null,
  },
  {
    name: "credential/locked",
    diagnostic: credentialStoreFailure("locked", "unlock_store"),
    message: {
      en: "Unlock the credential store and try again.",
      pt: "Desbloqueie o armazenamento de credenciais e tente novamente.",
    },
    // Replacing the provider key would not help: the sentence is the step.
    recovery: null,
  },
];
