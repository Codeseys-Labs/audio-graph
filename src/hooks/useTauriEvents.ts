/**
 * Tauri backend-event bridge.
 *
 * Call `useTauriEvents()` once at the root (see `App.tsx`). The hook
 * subscribes to every backend event the app cares about and funnels
 * each payload into the Zustand store or into a side-effect publisher:
 *
 *   - `TRANSCRIPT_UPDATE`       → `addTranscriptSegment`
 *   - `ASR_PARTIAL`             → `setAsrPartial`
 *   - `ASR_SPAN_REVISION`       → `addAsrSpanRevision`
 *   - `DIARIZATION_SPAN_REVISION`
 *                                → `addDiarizationSpanRevision`
 *   - `TURN_EVENT`              → `addTurnEvent`
 *   - `AGENT_CARD_UPDATE`       → `upsertLiveAssistCard` (fix round,
 *                                 audio-graph-83cc T5: wires the listener
 *                                 the backend's `CardAnswer` terminal-frame
 *                                 emission has always assumed exists — see
 *                                 `src-tauri/src/events.rs`'s doc on
 *                                 `AGENT_CARD_UPDATE`); dropped when its
 *                                 session is not the applied snapshot's,
 *                                 or no snapshot is applied yet
 *   - `GRAPH_UPDATE`            → `setGraphSnapshot`
 *   - `GRAPH_DELTA`             → `applyGraphDelta`
 *   - `MATERIALIZED_NOTES_UPDATE`
 *                                → `setMaterializedNotes` (session-fenced)
 *   - `MATERIALIZED_GRAPH_UPDATE`
 *                                → `setMaterializedProjectionGraph`
 *                                  (session-fenced)
 *   - `PIPELINE_STATUS`         → `setPipelineStatus`, applied only when
 *                                 newer than the applied snapshot; read
 *                                 again on focus, on visibility changes and
 *                                 every few seconds while capture is not
 *                                 idle, a Start is in flight or the state
 *                                 is unknown (before the first snapshot too)
 *   - `AUDIO_CONSUMER_HEALTH`   → `setAudioConsumerHealth`
 *   - `SPEAKER_DETECTED`        → `addOrUpdateSpeaker`
 *   - `CAPTURE_ERROR`           → `setError`, unless an older capture's
 *   - `PERSISTENCE_QUEUE_BACKPRESSURE`
 *                                → `setPersistenceQueueBackpressure`
 *   - `CAPTURE_STORAGE_FULL`    → `publishStorageFull` (StorageBanner)
 *   - `GEMINI_TRANSCRIPTION`    → `addGeminiTranscript`
 *   - `GEMINI_RESPONSE`         → `addGeminiTranscript`
 *   - `MODEL_DOWNLOAD_PROGRESS` → `downloadProgress` store slice
 *   - `PIPELINE_LATENCY`        → latest latency sample per pipeline stage
 *   - `GEMINI_STATUS`           → classified toast + store update
 *   - `AWS_ERROR`               → `setError` (localized via
 *                                 `awsErrorToMessage`)
 *
 * The event names are duplicated here as top-of-file string constants
 * so tests can assert on them; they must stay in sync with the Rust
 * constants in `src-tauri/src/events.rs`.
 *
 * Error-routing helpers `routeRealtimeDiagnostic` and `awsErrorToMessage`
 * are exported so unit tests and potential future diagnostics surfaces can
 * reuse the exact same classification without duplicating the switch
 * statements.
 */

import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { publishStorageFull } from "../components/StorageBanner";
import { CREDENTIAL_CONTRACT } from "../generated/credentialContract";
import {
  RUNTIME_DIAGNOSTIC_CONTRACT,
  type RuntimeErrorCode,
} from "../generated/runtimeDiagnostic";
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import { compareCaptureIds, sessionAdmits } from "../store/captureFacts";
import type {
  AgentProposalEvent,
  AgentStatusEvent,
  AsrPartialEvent,
  AsrSpanRevisionEvent,
  AwsErrorPayload,
  CaptureErrorPayload,
  CaptureStorageFullPayload,
  DiarizationSpanRevisionEvent,
  DownloadProgress,
  GeminiResponseEvent,
  GeminiStatusEvent,
  GeminiTranscriptionEvent,
  GraphDelta,
  GraphSnapshot,
  LiveAssistCardRecord,
  MaterializedGraph,
  MaterializedNotes,
  NotificationSeverity,
  OpenAiRealtimeResponseEvent,
  OpenAiRealtimeStatusEvent,
  PersistenceQueueBackpressurePayload,
  PipelineLatencyEvent,
  PipelineStatus,
  ProcessedAudioConsumerHealthPayload,
  ProjectionPatch,
  SpeakerInfo,
  TranscriptSegment,
  TurnLifecycleEvent,
} from "../types";
import { runtimeDiagnosticPresentation } from "../utils/runtimeDiagnostic";

/**
 * Route a realtime S2S engine error (issue #145 / ag#10 successor).
 *
 * The backend attaches a closed `RuntimeDiagnostic` envelope to every
 * Gemini Live / OpenAI Realtime status event of type `"error"` (and to the
 * AppError the start commands return). This helper normalizes the payload
 * through `runtimeDiagnosticPresentation` — fail-closed to the fixed
 * internal tuple for malformed values — and produces only a closed
 * translation key + toast severity. No raw provider text, WebSocket close
 * reason, request id, or exact length ever reaches a toast or the sticky
 * error banner.
 */
export function routeRealtimeDiagnostic(diagnostic: unknown): {
  key: string;
  variant: NotificationSeverity;
} {
  const presentation = runtimeDiagnosticPresentation(diagnostic);
  return {
    key: presentation.translationKey,
    variant:
      presentation.diagnostic.kind === "runtime"
        ? REALTIME_SEVERITY_BY_CODE[presentation.diagnostic.detail.code]
        : "error",
  };
}

/**
 * Toast severity by CLOSED runtime error code (issue #145). Keyed by the
 * closed vocabulary — not by `retryable`/recovery action — so a rate limit
 * stays a warning and a provider outage stays an error even though both are
 * retryable. Exhaustive (`Record`), so a new code fails type-check until
 * routed.
 *   rate_limited / request_rejected / policy_blocked / unsupported
 *                       → warning (user action or wait, not a crash)
 *   network_unreachable / timeout / cancelled
 *                       → info    (likely transient; reconnect retries)
 *   credential_rejected / credential_expired / permission_denied /
 *   invalid_configuration
 *                       → error   (the session cannot proceed until the user
 *                                  fixes credentials or settings; never info)
 *   everything else     → error   (genuinely broken)
 */
const REALTIME_SEVERITY_BY_CODE: Record<
  RuntimeErrorCode,
  NotificationSeverity
> = {
  rate_limited: "warning",
  request_rejected: "warning",
  policy_blocked: "warning",
  unsupported: "warning",
  network_unreachable: "info",
  timeout: "info",
  cancelled: "info",
  credential_rejected: "error",
  credential_expired: "error",
  permission_denied: "error",
  invalid_configuration: "error",
  provider_unavailable: "error",
  capacity_exhausted: "error",
  protocol_error: "error",
  invalid_response: "error",
  internal: "error",
};

// Event name constants — must match src-tauri/src/events.rs
const TRANSCRIPT_UPDATE = "transcript-update";
const ASR_PARTIAL = "asr-partial";
const ASR_SPAN_REVISION = "asr-span-revision";
const DIARIZATION_SPAN_REVISION = "diarization-span-revision";
const TURN_EVENT = "turn-event";
const AGENT_STATUS = "agent-status";
const AGENT_PROPOSAL = "agent-proposal";
const AGENT_CARD_UPDATE = "agent-card-update";
const GRAPH_UPDATE = "graph-update";
const GRAPH_DELTA = "graph-delta";
const PROJECTION_PATCH = "projection-patch";
const MATERIALIZED_NOTES_UPDATE = "materialized-notes-update";
const MATERIALIZED_GRAPH_UPDATE = "materialized-graph-update";
const PIPELINE_STATUS = "pipeline-status";
const AUDIO_CONSUMER_HEALTH = "audio-consumer-health";
const SPEAKER_DETECTED = "speaker-detected";
const CAPTURE_ERROR = "capture-error";
const PERSISTENCE_QUEUE_BACKPRESSURE = "persistence-queue-backpressure";
const CAPTURE_STORAGE_FULL = "capture-storage-full";
const GEMINI_TRANSCRIPTION = "gemini-transcription";
const GEMINI_RESPONSE = "gemini-response";
const GEMINI_STATUS = "gemini-status";
const OPENAI_REALTIME_RESPONSE = "openai-realtime-response";
const OPENAI_REALTIME_STATUS = "openai-realtime-status";
const MODEL_DOWNLOAD_PROGRESS = "model-download-progress";
const PIPELINE_LATENCY = "pipeline-latency";
const AWS_ERROR = "aws-error";

/**
 * How often the backend's state is read again while the window cannot vouch
 * for an idle backend: the shown capture is not idle, a Start is in flight,
 * or the state is unknown (including before the first snapshot, so a failed
 * attach or initial read is retried without a focus change).
 */
export const CAPTURE_RECONCILE_MS = 3_000;

// Streaming-chat token deltas + terminal frame no longer arrive as
// `chat-token-delta` / `chat-token-done` events: audio-graph-1534 moved that
// per-token hot path onto a per-invocation `tauri::ipc::Channel<ChatStreamEvent>`
// created and consumed inside the store's `sendChatMessage` (which also owns
// the 33ms delta coalescer that used to live here). This hook no longer
// subscribes to those events.

/**
 * Translate a structured {@link AwsErrorPayload} (ag#13) into a user-facing
 * message via the `aws.error.*` i18n namespace. Exported so unit tests and
 * any future in-app diagnostics panel can share the exact same mapping
 * without duplicating the switch.
 */
/**
 * Localize the detail of an `unknown` AWS error, or return `null`.
 *
 * On the Transcribe path the backend fills `UiAwsError::Unknown.message` with
 * the `Display` of a closed diagnostic — `runtime/<code>` or
 * `credential/<code>`. Only that exact shape, with a code from the generated
 * contract vocabulary, is translated. Anything else is untrusted: an older
 * producer can still put provider text there (#148), so it is never rendered
 * and the caller falls back to fixed copy.
 */
function awsUnknownDetail(message: string): string | null {
  const match = /^(runtime|credential)\/([a-z_]+)$/.exec(message);
  if (match === null) return null;
  const [, kind, code] = match;
  const codes: readonly string[] =
    kind === "runtime"
      ? RUNTIME_DIAGNOSTIC_CONTRACT.error_codes
      : CREDENTIAL_CONTRACT.vocabulary.error_codes;
  if (!codes.includes(code)) return null;
  const key = `runtimeDiagnostics.${kind}.${code}`;
  return i18n.exists(key) ? i18n.t(key) : null;
}

export function awsErrorToMessage(payload: AwsErrorPayload): string {
  const { error } = payload;
  switch (error.category) {
    case "invalid_access_key":
      return i18n.t("aws.error.invalidAccessKey");
    case "signature_mismatch":
      return i18n.t("aws.error.signatureMismatch");
    case "expired_token":
      return i18n.t("aws.error.expiredToken");
    case "access_denied":
      return i18n.t("aws.error.accessDenied", {
        // `permission` is `null` when the backend could not parse
        // the action out of the AWS message — the i18n copy falls
        // back to a generic "check your IAM policy" hint.
        permission: error.permission ?? "",
      });
    case "region_not_supported":
      return i18n.t("aws.error.regionNotSupported", {
        region: error.region,
      });
    case "network_unreachable":
      return i18n.t("aws.error.networkUnreachable");
    case "unknown": {
      const detail = awsUnknownDetail(error.message);
      return detail === null
        ? i18n.t("aws.error.unknownGeneric")
        : i18n.t("aws.error.unknown", { message: detail });
    }
  }
}

/**
 * Hook that subscribes to all Tauri backend events and updates the Zustand store.
 * Should be called once at the app root level.
 */
export function useTauriEvents(): void {
  const addTranscriptSegment = useAudioGraphStore(
    (s) => s.addTranscriptSegment,
  );
  const setAsrPartial = useAudioGraphStore((s) => s.setAsrPartial);
  const addAsrSpanRevision = useAudioGraphStore((s) => s.addAsrSpanRevision);
  const addDiarizationSpanRevision = useAudioGraphStore(
    (s) => s.addDiarizationSpanRevision,
  );
  const addTurnEvent = useAudioGraphStore((s) => s.addTurnEvent);
  const setAgentStatus = useAudioGraphStore((s) => s.setAgentStatus);
  const addAgentProposal = useAudioGraphStore((s) => s.addAgentProposal);
  const upsertLiveAssistCard = useAudioGraphStore(
    (s) => s.upsertLiveAssistCard,
  );
  const setGraphSnapshot = useAudioGraphStore((s) => s.setGraphSnapshot);
  const applyGraphDelta = useAudioGraphStore((s) => s.applyGraphDelta);
  const addProjectionPatch = useAudioGraphStore((s) => s.addProjectionPatch);
  const setMaterializedNotes = useAudioGraphStore(
    (s) => s.setMaterializedNotes,
  );
  const setMaterializedProjectionGraph = useAudioGraphStore(
    (s) => s.setMaterializedProjectionGraph,
  );
  const setPipelineStatus = useAudioGraphStore((s) => s.setPipelineStatus);
  const setPipelineLatency = useAudioGraphStore((s) => s.setPipelineLatency);
  const setAudioConsumerHealth = useAudioGraphStore(
    (s) => s.setAudioConsumerHealth,
  );
  const addOrUpdateSpeaker = useAudioGraphStore((s) => s.addOrUpdateSpeaker);
  const setError = useAudioGraphStore((s) => s.setError);
  const notify = useAudioGraphStore((s) => s.notify);
  const setPersistenceQueueBackpressure = useAudioGraphStore(
    (s) => s.setPersistenceQueueBackpressure,
  );
  const addGeminiTranscript = useAudioGraphStore((s) => s.addGeminiTranscript);

  // While the shown capture is pending, running or stopping, a Start is in
  // flight, or the state is unknown (no snapshot applied since load, or a
  // command's outcome lost), it is read again every few seconds: a lost
  // emission, a failed attach or a failed first read is never shown for
  // long, focus or not. One interval, cleared once idle and on unmount.
  const reconcilePeriodically = useAudioGraphStore(
    (s) =>
      s.captureStatus !== "idle" || s.captureUnknown || s.startPending !== null,
  );
  useEffect(() => {
    if (!reconcilePeriodically) return;
    const interval = setInterval(() => {
      void useAudioGraphStore.getState().reconcileCapture();
    }, CAPTURE_RECONCILE_MS);
    return () => clearInterval(interval);
  }, [reconcilePeriodically]);

  useEffect(() => {
    let unlisten: Array<(() => void) | null> = [];
    // H6: cleanup may run before the async set() resolves. Track a cancelled
    // flag so listeners that resolve after unmount are unlistened instead of
    // leaking (the sync cleanup below would otherwise iterate an empty array).
    let cancelled = false;

    // H5: coalesce high-frequency REPLACE-semantics events (latest wins) so
    // a flood triggers ~10fps store updates instead of one per event. Only
    // safe for events where intermediate values can be dropped — NOT
    // graph-delta (cumulative; dropping would lose nodes) or transcript
    // (low-frequency, each is meaningful).
    const EVENT_THROTTLE_MS = 100;
    const latestThrottles: Array<{ cancel: () => void }> = [];
    function latestThrottle<T>(apply: (p: T) => void, ms: number) {
      let latest: T | null = null;
      let has = false;
      let timer: ReturnType<typeof setTimeout> | null = null;
      const flush = () => {
        timer = null;
        if (!has) return;
        has = false;
        const payload = latest as T;
        latest = null;
        apply(payload);
      };
      const t = {
        push: (p: T) => {
          latest = p;
          has = true;
          if (timer === null) timer = setTimeout(flush, ms);
        },
        cancel: () => {
          if (timer !== null) {
            clearTimeout(timer);
            timer = null;
          }
        },
      };
      latestThrottles.push(t);
      return t;
    }
    const asrPartialThrottle = latestThrottle<AsrPartialEvent>(
      setAsrPartial,
      EVENT_THROTTLE_MS,
    );
    const latencyThrottle = latestThrottle<PipelineLatencyEvent>(
      setPipelineLatency,
      EVENT_THROTTLE_MS,
    );

    async function safeListen<T>(
      eventName: string,
      cb: (event: { payload: T }) => void,
    ): Promise<(() => void) | null> {
      try {
        return await listen<T>(eventName, cb as never);
      } catch (err) {
        console.error(`Failed to subscribe to ${eventName}:`, err);
        return null;
      }
    }

    async function setup() {
      const handles = await Promise.all([
        safeListen<TranscriptSegment>(TRANSCRIPT_UPDATE, (event) => {
          addTranscriptSegment(event.payload);
        }),
        safeListen<AsrPartialEvent>(ASR_PARTIAL, (event) => {
          asrPartialThrottle.push(event.payload);
        }),
        safeListen<AsrSpanRevisionEvent>(ASR_SPAN_REVISION, (event) => {
          addAsrSpanRevision(event.payload);
        }),
        safeListen<DiarizationSpanRevisionEvent>(
          DIARIZATION_SPAN_REVISION,
          (event) => {
            addDiarizationSpanRevision(event.payload);
          },
        ),
        safeListen<TurnLifecycleEvent>(TURN_EVENT, (event) => {
          addTurnEvent(event.payload);
        }),
        safeListen<AgentStatusEvent>(AGENT_STATUS, (event) => {
          setAgentStatus(event.payload);
        }),
        safeListen<AgentProposalEvent>(AGENT_PROPOSAL, (event) => {
          addAgentProposal(event.payload);
          notify({
            severity: event.payload.kind === "question" ? "info" : "success",
            message: event.payload.title,
          });
        }),
        // audio-graph-83cc T5 (fix round): the backend has emitted this
        // since T3/T3G's `finalize_card_answer`, but nothing consumed it —
        // deliverable (d)'s "answered ⇒ feed" classification rule was
        // unreachable in production without this listener.
        safeListen<LiveAssistCardRecord>(AGENT_CARD_UPDATE, (event) => {
          // A card of another session than the shown one is stale; before
          // any snapshot no session is shown, and the card is dropped.
          const { pipelineStatus } = useAudioGraphStore.getState();
          if (!sessionAdmits(pipelineStatus, event.payload.session_id)) return;
          upsertLiveAssistCard(event.payload);
        }),
        safeListen<GraphSnapshot>(GRAPH_UPDATE, (event) => {
          setGraphSnapshot(event.payload);
        }),
        safeListen<GraphDelta>(GRAPH_DELTA, (event) => {
          applyGraphDelta(event.payload);
        }),
        safeListen<ProjectionPatch>(PROJECTION_PATCH, (event) => {
          addProjectionPatch(event.payload);
        }),
        safeListen<MaterializedNotes>(MATERIALIZED_NOTES_UPDATE, (event) => {
          const { pipelineStatus } = useAudioGraphStore.getState();
          if (!sessionAdmits(pipelineStatus, event.payload.session_id)) return;
          setMaterializedNotes(event.payload);
        }),
        safeListen<MaterializedGraph>(MATERIALIZED_GRAPH_UPDATE, (event) => {
          const { pipelineStatus } = useAudioGraphStore.getState();
          if (!sessionAdmits(pipelineStatus, event.payload.session_id)) return;
          setMaterializedProjectionGraph(event.payload);
        }),
        safeListen<PipelineStatus>(PIPELINE_STATUS, (event) => {
          setPipelineStatus(event.payload);
        }),
        safeListen<ProcessedAudioConsumerHealthPayload>(
          AUDIO_CONSUMER_HEALTH,
          (event) => {
            setAudioConsumerHealth(event.payload);
          },
        ),
        safeListen<SpeakerInfo>(SPEAKER_DETECTED, (event) => {
          addOrUpdateSpeaker(event.payload);
        }),
        safeListen<CaptureErrorPayload>(CAPTURE_ERROR, (event) => {
          // The payload's source id can carry an application name: log only
          // the closed recoverability flag.
          console.error("Capture error", {
            recoverable: event.payload.recoverable,
          });
          // An error from a capture older than the one the applied snapshot
          // shows is stale.
          const shown =
            useAudioGraphStore.getState().pipelineStatus.capture_state.id;
          if (compareCaptureIds(event.payload.capture_id, shown) >= 0) {
            setError(event.payload.error);
          }
        }),
        safeListen<PersistenceQueueBackpressurePayload>(
          PERSISTENCE_QUEUE_BACKPRESSURE,
          (event) => {
            setPersistenceQueueBackpressure(event.payload);
          },
        ),
        safeListen<CaptureStorageFullPayload>(CAPTURE_STORAGE_FULL, (event) => {
          console.error("Storage full:", event.payload);
          publishStorageFull(event.payload);
        }),
        safeListen<GeminiTranscriptionEvent>(GEMINI_TRANSCRIPTION, (event) => {
          const { text, is_final } = event.payload;
          addGeminiTranscript({
            id: `gemini-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
            text,
            timestamp: Date.now(),
            is_final,
            source: "gemini",
          });
        }),
        safeListen<GeminiResponseEvent>(GEMINI_RESPONSE, (event) => {
          addGeminiTranscript({
            id: `gemini-resp-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
            text: `[Gemini] ${event.payload.text}`,
            timestamp: Date.now(),
            is_final: true,
            source: "gemini",
          });
        }),
        safeListen<DownloadProgress>(MODEL_DOWNLOAD_PROGRESS, (event) => {
          useAudioGraphStore.setState({
            downloadProgress: event.payload,
          });
        }),
        safeListen<PipelineLatencyEvent>(PIPELINE_LATENCY, (event) => {
          latencyThrottle.push(event.payload);
        }),
        safeListen<GeminiStatusEvent>(GEMINI_STATUS, (event) => {
          const { type: statusType, diagnostic, resumed } = event.payload;
          if (statusType === "error") {
            // Issue #145: route strictly through the closed
            // `RuntimeDiagnostic` envelope. `runtimeDiagnosticPresentation`
            // fail-closes malformed payloads to the fixed internal tuple,
            // so the raw `message` string (a closed site token) is never
            // consulted and no provider text, close reason, or request id
            // can reach a toast or the sticky error banner.
            const { key, variant } = routeRealtimeDiagnostic(diagnostic);
            notify({
              severity: variant,
              message: i18n.t(key),
            });
          } else if (statusType === "reconnected") {
            // Recovery clears the sticky legacy error banner (FINDING #56
            // P2) — nothing else clears it, so it would otherwise linger
            // long after the link is healthy again. Whether the mode runs
            // is the snapshot's `realtime`, not this notification's.
            setError(null);
            notify({
              severity: resumed ? "success" : "info",
              message: i18n.t(
                resumed ? "gemini.reconnect.resumed" : "gemini.reconnect.fresh",
              ),
            });
          } else if (statusType === "connected") {
            // A fresh successful connection likewise supersedes any stale
            // error banner left by a prior failed attempt (FINDING #56 P2).
            setError(null);
          }
        }),
        safeListen<OpenAiRealtimeResponseEvent>(
          OPENAI_REALTIME_RESPONSE,
          (event) => {
            addGeminiTranscript({
              id: `openai-rt-resp-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
              text: `[OpenAI] ${event.payload.text}`,
              timestamp: Date.now(),
              is_final: event.payload.final,
              source: "openai-realtime",
            });
          },
        ),
        safeListen<OpenAiRealtimeStatusEvent>(
          OPENAI_REALTIME_STATUS,
          (event) => {
            const { type: statusType, diagnostic, resumed } = event.payload;
            if (statusType === "error") {
              // Issue #145: same closed-envelope routing as the Gemini
              // handler above — no raw `message` fallback, no provider
              // text, close reasons, or request ids.
              const { key, variant } = routeRealtimeDiagnostic(diagnostic);
              notify({
                severity: variant,
                message: i18n.t(key),
              });
            } else if (statusType === "reconnected") {
              setError(null);
              notify({
                severity: resumed ? "success" : "info",
                message: i18n.t(
                  resumed
                    ? "gemini.reconnect.resumed"
                    : "gemini.reconnect.fresh",
                ),
              });
            } else if (statusType === "connected") {
              setError(null);
            }
          },
        ),
        safeListen<AwsErrorPayload>(AWS_ERROR, (event) => {
          console.error("AWS error:", event.payload);
          // Route structured AWS errors through the error banner
          // (same UI path as other blocking errors) with a
          // localized, actionable message built from the
          // category-specific i18n key.
          setError(awsErrorToMessage(event.payload));
        }),
        // Streaming-chat deltas/done moved to a per-invocation
        // `tauri::ipc::Channel` consumed in the store's sendChatMessage
        // (audio-graph-1534) — no `chat-token-delta` / `chat-token-done`
        // listeners here anymore.
      ]);
      if (cancelled) {
        // Unmounted before listeners resolved — unlisten them now so
        // they don't leak (H6).
        for (const fn of handles) {
          if (fn) fn();
        }
        return;
      }
      // Lost emissions are bounded: a window that regains focus or changes
      // visibility reads the backend's state again.
      const reconcile = () => {
        void useAudioGraphStore.getState().reconcileCapture();
      };
      window.addEventListener("focus", reconcile);
      document.addEventListener("visibilitychange", reconcile);
      unlisten = [
        ...handles,
        () => {
          window.removeEventListener("focus", reconcile);
          document.removeEventListener("visibilitychange", reconcile);
        },
      ];
      // Attach to capture control once this realm listens: the snapshot it
      // returns adopts a capture still running after a reload, and the
      // attachment cancels any start the previous page left pending. The
      // store keeps one attachment per realm, so a remount reuses it.
      useAudioGraphStore.getState().attachCapture();
      // The first status read comes after the subscription, so nothing
      // published in between is lost; like every snapshot it is applied
      // only when newer than the one already applied.
      reconcile();
    }

    setup();

    return () => {
      cancelled = true;
      for (const t of latestThrottles) {
        t.cancel();
      }
      for (const fn of unlisten) {
        if (fn) fn();
      }
    };
  }, [
    addTranscriptSegment,
    setAsrPartial,
    addAsrSpanRevision,
    addDiarizationSpanRevision,
    addTurnEvent,
    setAgentStatus,
    addAgentProposal,
    upsertLiveAssistCard,
    setGraphSnapshot,
    applyGraphDelta,
    addProjectionPatch,
    setMaterializedNotes,
    setMaterializedProjectionGraph,
    setPipelineStatus,
    setPipelineLatency,
    setAudioConsumerHealth,
    addOrUpdateSpeaker,
    setError,
    setPersistenceQueueBackpressure,
    addGeminiTranscript,
    notify,
  ]);
}
