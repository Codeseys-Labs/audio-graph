/**
 * Tauri backend-event bridge.
 *
 * Call `useTauriEvents()` once at the root (see `App.tsx`). The hook
 * subscribes to every backend event the app cares about and funnels
 * each payload into the Zustand store or into a side-effect publisher:
 *
 *   - `TRANSCRIPT_UPDATE`       → `addTranscriptSegment`
 *   - `ASR_PARTIAL`             → `setAsrPartial`
 *   - `TURN_EVENT`              → `addTurnEvent`
 *   - `AGENT_STATUS`            → `setAgentStatus`
 *   - `AGENT_PROPOSAL`          → `addAgentProposal` + toast
 *   - `GRAPH_UPDATE`            → `setGraphSnapshot`
 *   - `GRAPH_DELTA`             → `applyGraphDelta`
 *   - `PIPELINE_STATUS`         → `setPipelineStatus`
 *   - `SPEAKER_DETECTED`        → `addOrUpdateSpeaker`
 *   - `CAPTURE_ERROR`           → `setError`
 *   - `CAPTURE_BACKPRESSURE`    → `setSourceBackpressure`
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
 * Error-routing helpers `routeGeminiError` and `awsErrorToMessage` are
 * exported so unit tests and potential future diagnostics surfaces can
 * reuse the exact same classification without duplicating the switch
 * statements.
 */

import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { publishStorageFull } from "../components/StorageBanner";
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import type {
  AgentProposalEvent,
  AgentStatusEvent,
  AsrPartialEvent,
  AwsErrorPayload,
  CaptureBackpressurePayload,
  CaptureErrorPayload,
  CaptureStorageFullPayload,
  ChatTokenDeltaEvent,
  ChatTokenDoneEvent,
  DownloadProgress,
  GeminiErrorCategory,
  GeminiResponseEvent,
  GeminiStatusEvent,
  GeminiTranscriptionEvent,
  GraphDelta,
  GraphSnapshot,
  NotificationSeverity,
  PipelineLatencyEvent,
  PipelineStatus,
  SpeakerInfo,
  TranscriptSegment,
  TurnLifecycleEvent,
} from "../types";

/**
 * Map a classified Gemini error category to its i18n key + toast variant.
 *
 * Routing rules (ag#10 spec):
 *   auth, auth_expired, rate_limit → warning (user action required,
 *                                             not a crash)
 *   network                        → info     (likely transient; the
 *                                              reconnect loop will retry)
 *   server, unknown                → error    (genuinely broken)
 *
 * Keys live under `gemini.error.*` so translators can group them.
 */
export function routeGeminiError(category: GeminiErrorCategory): {
  key: string;
  variant: NotificationSeverity;
} {
  switch (category.kind) {
    case "auth":
      return { key: "gemini.error.auth", variant: "warning" };
    case "auth_expired":
      return { key: "gemini.error.authExpired", variant: "warning" };
    case "rate_limit":
      return { key: "gemini.error.rateLimit", variant: "warning" };
    case "network":
      return { key: "gemini.error.network", variant: "info" };
    case "server":
      return { key: "gemini.error.server", variant: "error" };
    case "unknown":
    default:
      return { key: "gemini.error.unknown", variant: "error" };
  }
}

// Event name constants — must match src-tauri/src/events.rs
const TRANSCRIPT_UPDATE = "transcript-update";
const ASR_PARTIAL = "asr-partial";
const TURN_EVENT = "turn-event";
const AGENT_STATUS = "agent-status";
const AGENT_PROPOSAL = "agent-proposal";
const GRAPH_UPDATE = "graph-update";
const GRAPH_DELTA = "graph-delta";
const PIPELINE_STATUS = "pipeline-status";
const SPEAKER_DETECTED = "speaker-detected";
const CAPTURE_ERROR = "capture-error";
const CAPTURE_BACKPRESSURE = "capture-backpressure";
const CAPTURE_STORAGE_FULL = "capture-storage-full";
const GEMINI_TRANSCRIPTION = "gemini-transcription";
const GEMINI_RESPONSE = "gemini-response";
const GEMINI_STATUS = "gemini-status";
const MODEL_DOWNLOAD_PROGRESS = "model-download-progress";
const PIPELINE_LATENCY = "pipeline-latency";
const AWS_ERROR = "aws-error";
const CHAT_TOKEN_DELTA = "chat-token-delta";
const CHAT_TOKEN_DONE = "chat-token-done";

/**
 * Streaming-chat coalescing window. Tokens arrive at variable rates from
 * the LLM provider — bursts of 50+ deltas in a single frame are common
 * mid-stream. Without coalescing, every delta fires `appendChatTokenDelta`
 * which triggers a Zustand subscriber notification + React re-render. At
 * full rate that thrashes layout. We batch deltas into the store at most
 * once per 33 ms (~30 fps), which is below the human flicker threshold
 * but well above the burst rate. Configurable here for unit tests.
 */
const CHAT_DELTA_THROTTLE_MS = 33;

/**
 * Translate a structured {@link AwsErrorPayload} (ag#13) into a user-facing
 * message via the `aws.error.*` i18n namespace. Exported so unit tests and
 * any future in-app diagnostics panel can share the exact same mapping
 * without duplicating the switch.
 */
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
    case "unknown":
      return i18n.t("aws.error.unknown", { message: error.message });
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
  const addTurnEvent = useAudioGraphStore((s) => s.addTurnEvent);
  const setAgentStatus = useAudioGraphStore((s) => s.setAgentStatus);
  const addAgentProposal = useAudioGraphStore((s) => s.addAgentProposal);
  const setGraphSnapshot = useAudioGraphStore((s) => s.setGraphSnapshot);
  const applyGraphDelta = useAudioGraphStore((s) => s.applyGraphDelta);
  const setPipelineStatus = useAudioGraphStore((s) => s.setPipelineStatus);
  const setPipelineLatency = useAudioGraphStore((s) => s.setPipelineLatency);
  const addOrUpdateSpeaker = useAudioGraphStore((s) => s.addOrUpdateSpeaker);
  const setError = useAudioGraphStore((s) => s.setError);
  const notify = useAudioGraphStore((s) => s.notify);
  const setSourceBackpressure = useAudioGraphStore(
    (s) => s.setSourceBackpressure,
  );
  const addGeminiTranscript = useAudioGraphStore((s) => s.addGeminiTranscript);
  const appendChatTokenDelta = useAudioGraphStore(
    (s) => s.appendChatTokenDelta,
  );
  const finalizeChatStream = useAudioGraphStore((s) => s.finalizeChatStream);

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

    // Streaming-chat delta coalescer: queue raw deltas as they arrive
    // and flush them into the store at most once per
    // CHAT_DELTA_THROTTLE_MS. The flush concatenates contiguous
    // deltas for the same request_id into a single store update so a
    // burst of N tokens triggers one re-render, not N. The terminal
    // `chat-token-done` event drains the queue synchronously before
    // calling finalizeChatStream, so the in-progress message can't
    // observe a Done that pre-empts a queued Delta.
    let pendingDeltas: ChatTokenDeltaEvent[] = [];
    let flushTimer: ReturnType<typeof setTimeout> | null = null;
    const flushDeltas = () => {
      flushTimer = null;
      if (pendingDeltas.length === 0) return;
      const batch = pendingDeltas;
      pendingDeltas = [];
      // Group by request_id (typically one) and concatenate the
      // delta strings to minimize the number of store updates.
      const grouped = new Map<string, string>();
      const finishReasons = new Map<string, string | undefined>();
      for (const d of batch) {
        grouped.set(d.request_id, (grouped.get(d.request_id) ?? "") + d.delta);
        if (d.finish_reason) {
          finishReasons.set(d.request_id, d.finish_reason);
        }
      }
      for (const [requestId, delta] of grouped) {
        appendChatTokenDelta({
          request_id: requestId,
          delta,
          finish_reason: finishReasons.get(requestId),
        });
      }
    };
    const scheduleFlush = () => {
      if (flushTimer !== null) return;
      flushTimer = setTimeout(flushDeltas, CHAT_DELTA_THROTTLE_MS);
    };
    const drainNow = () => {
      if (flushTimer !== null) {
        clearTimeout(flushTimer);
        flushTimer = null;
      }
      flushDeltas();
    };

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
        safeListen<GraphSnapshot>(GRAPH_UPDATE, (event) => {
          setGraphSnapshot(event.payload);
        }),
        safeListen<GraphDelta>(GRAPH_DELTA, (event) => {
          applyGraphDelta(event.payload);
        }),
        safeListen<PipelineStatus>(PIPELINE_STATUS, (event) => {
          setPipelineStatus(event.payload);
        }),
        safeListen<SpeakerInfo>(SPEAKER_DETECTED, (event) => {
          addOrUpdateSpeaker(event.payload);
        }),
        safeListen<CaptureErrorPayload>(CAPTURE_ERROR, (event) => {
          console.error("Capture error:", event.payload);
          setError(event.payload.error);
        }),
        safeListen<CaptureBackpressurePayload>(
          CAPTURE_BACKPRESSURE,
          (event) => {
            const { source_id, is_backpressured } = event.payload;
            setSourceBackpressure(source_id, is_backpressured);
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
          const {
            type: statusType,
            message,
            resumed,
            category,
          } = event.payload;
          if (statusType === "error") {
            // Structured routing: prefer the classified
            // `category` (ag#10) to pick the i18n key + toast
            // severity. Fall back to the raw `message` in the
            // error banner for unclassified or legacy events.
            if (category) {
              const { key, variant } = routeGeminiError(category);
              const extra =
                category.kind === "rate_limit" &&
                typeof category.retry_after_secs === "number"
                  ? { retry: category.retry_after_secs }
                  : undefined;
              notify({
                severity: variant,
                message: i18n.t(key, extra),
              });
            } else if (message) {
              setError(`Gemini: ${message}`);
            }
          } else if (statusType === "disconnected") {
            useAudioGraphStore.setState({ isGeminiActive: false });
          } else if (statusType === "reconnected") {
            notify({
              severity: resumed ? "success" : "info",
              message: i18n.t(
                resumed ? "gemini.reconnect.resumed" : "gemini.reconnect.fresh",
              ),
            });
          }
        }),
        safeListen<AwsErrorPayload>(AWS_ERROR, (event) => {
          console.error("AWS error:", event.payload);
          // Route structured AWS errors through the error banner
          // (same UI path as other blocking errors) with a
          // localized, actionable message built from the
          // category-specific i18n key.
          setError(awsErrorToMessage(event.payload));
        }),
        safeListen<ChatTokenDeltaEvent>(CHAT_TOKEN_DELTA, (event) => {
          // Coalesce: queue + schedule a flush instead of
          // touching the store on every delta. See the comment
          // by `pendingDeltas` for rationale.
          pendingDeltas.push(event.payload);
          scheduleFlush();
        }),
        safeListen<ChatTokenDoneEvent>(CHAT_TOKEN_DONE, (event) => {
          // Drain any queued deltas first so the assistant
          // message reflects everything we received before
          // finalizing with the authoritative full_text.
          drainNow();
          finalizeChatStream(event.payload);
        }),
      ]);
      if (cancelled) {
        // Unmounted before listeners resolved — unlisten them now so
        // they don't leak (H6).
        for (const fn of handles) {
          if (fn) fn();
        }
        return;
      }
      unlisten = handles;
    }

    setup();

    return () => {
      cancelled = true;
      if (flushTimer !== null) {
        clearTimeout(flushTimer);
        flushTimer = null;
      }
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
    addTurnEvent,
    setAgentStatus,
    addAgentProposal,
    setGraphSnapshot,
    applyGraphDelta,
    setPipelineStatus,
    setPipelineLatency,
    addOrUpdateSpeaker,
    setError,
    setSourceBackpressure,
    addGeminiTranscript,
    appendChatTokenDelta,
    finalizeChatStream,
  ]);
}
