import type {
  ProviderReadiness,
  SttFidelityDegradation,
  SttFidelityOrigin,
} from ".";

export const readyButDegradedFinalOnlyReadiness = {
  provider_id: "asr.api",
  status: "ready",
  message: "Provider health check succeeded",
  stale: false,
  credential_epoch: 1,
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
    ],
  },
} satisfies ProviderReadiness;

// @ts-expect-error transcript inspection is not a readiness fidelity origin
export const transcriptInferredOrigin: SttFidelityOrigin =
  "transcript_inferred";

// @ts-expect-error arbitrary diagnostic text is not a typed degradation code
export const untypedDegradation: SttFidelityDegradation = "low_fidelity";
