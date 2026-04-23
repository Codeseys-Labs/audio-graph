import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import i18n from "../i18n";
import { showToast } from "../components/Toast";
import { publishStorageFull } from "../components/StorageBanner";
import { useAudioGraphStore } from "../store";
import type {
    TranscriptSegment,
    GraphSnapshot,
    PipelineStatus,
    SpeakerInfo,
    CaptureErrorPayload,
    CaptureBackpressurePayload,
    CaptureStorageFullPayload,
    GeminiTranscriptionEvent,
    GeminiResponseEvent,
    GeminiStatusEvent,
} from "../types";

// Event name constants — must match src-tauri/src/events.rs
const TRANSCRIPT_UPDATE = "transcript-update";
const GRAPH_UPDATE = "graph-update";
const PIPELINE_STATUS = "pipeline-status";
const SPEAKER_DETECTED = "speaker-detected";
const CAPTURE_ERROR = "capture-error";
const CAPTURE_BACKPRESSURE = "capture-backpressure";
const CAPTURE_STORAGE_FULL = "capture-storage-full";
const GEMINI_TRANSCRIPTION = "gemini-transcription";
const GEMINI_RESPONSE = "gemini-response";
const GEMINI_STATUS = "gemini-status";

/**
 * Hook that subscribes to all Tauri backend events and updates the Zustand store.
 * Should be called once at the app root level.
 */
export function useTauriEvents(): void {
    const addTranscriptSegment = useAudioGraphStore((s) => s.addTranscriptSegment);
    const setGraphSnapshot = useAudioGraphStore((s) => s.setGraphSnapshot);
    const setPipelineStatus = useAudioGraphStore((s) => s.setPipelineStatus);
    const addOrUpdateSpeaker = useAudioGraphStore((s) => s.addOrUpdateSpeaker);
    const setError = useAudioGraphStore((s) => s.setError);
    const setSourceBackpressure = useAudioGraphStore((s) => s.setSourceBackpressure);
    const addGeminiTranscript = useAudioGraphStore((s) => s.addGeminiTranscript);

    useEffect(() => {
        let unlisten: Array<(() => void) | null> = [];

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
            unlisten = await Promise.all([
                safeListen<TranscriptSegment>(TRANSCRIPT_UPDATE, (event) => {
                    addTranscriptSegment(event.payload);
                }),
                safeListen<GraphSnapshot>(GRAPH_UPDATE, (event) => {
                    setGraphSnapshot(event.payload);
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
                safeListen<CaptureBackpressurePayload>(CAPTURE_BACKPRESSURE, (event) => {
                    const { source_id, is_backpressured } = event.payload;
                    setSourceBackpressure(source_id, is_backpressured);
                }),
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
                safeListen<GeminiStatusEvent>(GEMINI_STATUS, (event) => {
                    const { type: statusType, message, resumed } = event.payload;
                    if (statusType === "error" && message) {
                        setError(`Gemini: ${message}`);
                    } else if (statusType === "disconnected") {
                        useAudioGraphStore.setState({ isGeminiActive: false });
                    } else if (statusType === "reconnected") {
                        showToast({
                            variant: resumed ? "success" : "info",
                            message: i18n.t(
                                resumed
                                    ? "gemini.reconnect.resumed"
                                    : "gemini.reconnect.fresh",
                            ),
                        });
                    }
                }),
            ]);
        }

        setup();

        return () => {
            for (const fn of unlisten) {
                if (fn) fn();
            }
        };
    }, [
        addTranscriptSegment,
        setGraphSnapshot,
        setPipelineStatus,
        addOrUpdateSpeaker,
        setError,
        setSourceBackpressure,
        addGeminiTranscript,
    ]);
}
