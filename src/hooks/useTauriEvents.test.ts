import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { listen } from "@tauri-apps/api/event";
import type { Event } from "@tauri-apps/api/event";
import {
    useTauriEvents,
    awsErrorToMessage,
    routeGeminiError,
} from "./useTauriEvents";
import { showToast } from "../components/Toast";
import type { AwsErrorPayload, GeminiErrorCategory } from "../types";
import { useAudioGraphStore } from "../store";

vi.mock("../components/Toast", async () => {
    const actual = await vi.importActual<typeof import("../components/Toast")>(
        "../components/Toast",
    );
    return { ...actual, showToast: vi.fn() };
});

// The global setup (src/test/setup.ts) already mocks @tauri-apps/api/event
// with a `listen` that returns a no-op unlisten. Here we redefine its
// behavior per-test so we can capture handlers and assert payload routing.
type Handler = (event: Event<unknown>) => void;

function makeEvent<T>(name: string, payload: T): Event<T> {
    return { event: name, id: 0, payload } as Event<T>;
}

function resetStore() {
    useAudioGraphStore.setState({
        transcriptSegments: [],
        graphSnapshot: {
            nodes: [],
            links: [],
            stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
        },
        pipelineStatus: {
            capture: { type: "Idle" },
            pipeline: { type: "Idle" },
            asr: { type: "Idle" },
            diarization: { type: "Idle" },
            entity_extraction: { type: "Idle" },
            graph: { type: "Idle" },
        },
        speakers: [],
        backpressuredSources: [],
        geminiTranscripts: [],
        error: null,
        isGeminiActive: true,
    });
}

describe("useTauriEvents", () => {
    const handlers = new Map<string, Handler>();
    const unlisteners: Array<ReturnType<typeof vi.fn>> = [];

    beforeEach(() => {
        handlers.clear();
        unlisteners.length = 0;
        resetStore();

        vi.mocked(listen).mockImplementation(
            async (eventName: string, cb: Handler) => {
                handlers.set(eventName, cb);
                const unlisten = vi.fn();
                unlisteners.push(unlisten);
                return unlisten;
            },
        );
    });

    afterEach(() => {
        vi.clearAllMocks();
    });

    // The hook's setup() runs `TOTAL_LISTENERS` listen() calls concurrently
    // via Promise.all; waitFor polls until all handlers are present.
    //
    // When adding a new listener, update both this constant and the
    // `expected` list in the "subscribes to all expected events on mount"
    // test. The count is also exercised by the unlisten-cleanup test and
    // the partial-failure test (which drops exactly one).
    const TOTAL_LISTENERS = 12;
    async function waitForAllHandlers() {
        await waitFor(() => {
            expect(handlers.size).toBe(TOTAL_LISTENERS);
        });
    }

    it("subscribes to all expected events on mount", async () => {
        const { unmount } = renderHook(() => useTauriEvents());
        await waitForAllHandlers();

        const expected = [
            "transcript-update",
            "graph-update",
            "pipeline-status",
            "speaker-detected",
            "capture-error",
            "capture-backpressure",
            "capture-storage-full",
            "model-download-progress",
            "gemini-transcription",
            "gemini-response",
            "gemini-status",
            "aws-error",
        ];
        for (const name of expected) {
            expect(handlers.has(name)).toBe(true);
        }
        expect(handlers.size).toBe(expected.length);
        unmount();
    });

    it("invokes every registered unlisten on unmount", async () => {
        const { unmount } = renderHook(() => useTauriEvents());
        await waitForAllHandlers();

        const count = unlisteners.length;
        expect(count).toBe(TOTAL_LISTENERS);
        unmount();

        for (const fn of unlisteners) {
            expect(fn).toHaveBeenCalledTimes(1);
        }
    });

    it("routes transcript-update payload into the store", async () => {
        renderHook(() => useTauriEvents());
        await waitForAllHandlers();

        const segment = {
            id: "seg-1",
            speaker_id: "spk-1",
            text: "hello",
            start_time: 0,
            end_time: 1,
            confidence: 0.9,
        };
        handlers.get("transcript-update")?.(
            makeEvent("transcript-update", segment),
        );

        expect(useAudioGraphStore.getState().transcriptSegments).toEqual([
            segment,
        ]);
    });

    it("routes pipeline-status and speaker-detected payloads", async () => {
        renderHook(() => useTauriEvents());
        await waitForAllHandlers();

        const running = { type: "Running" } as const;
        const status = {
            capture: running,
            pipeline: running,
            asr: running,
            diarization: running,
            entity_extraction: running,
            graph: running,
        };
        handlers.get("pipeline-status")?.(makeEvent("pipeline-status", status));
        expect(useAudioGraphStore.getState().pipelineStatus).toEqual(status);

        const speaker = { id: "spk-1", label: "Alice", color: "#ff0000" };
        handlers.get("speaker-detected")?.(
            makeEvent("speaker-detected", speaker),
        );
        expect(useAudioGraphStore.getState().speakers).toContainEqual(speaker);
    });

    it("sets store.error from capture-error payload", async () => {
        const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
        renderHook(() => useTauriEvents());
        await waitForAllHandlers();

        handlers.get("capture-error")?.(
            makeEvent("capture-error", {
                source_id: "mic-1",
                error: "device disconnected",
            }),
        );

        expect(useAudioGraphStore.getState().error).toBe("device disconnected");
        errSpy.mockRestore();
    });

    it("tracks capture-backpressure add and clear transitions", async () => {
        renderHook(() => useTauriEvents());
        await waitForAllHandlers();

        handlers.get("capture-backpressure")?.(
            makeEvent("capture-backpressure", {
                source_id: "mic-1",
                is_backpressured: true,
            }),
        );
        expect(useAudioGraphStore.getState().backpressuredSources).toContain(
            "mic-1",
        );

        handlers.get("capture-backpressure")?.(
            makeEvent("capture-backpressure", {
                source_id: "mic-1",
                is_backpressured: false,
            }),
        );
        expect(useAudioGraphStore.getState().backpressuredSources).not.toContain(
            "mic-1",
        );
    });

    it("appends gemini-transcription events to the transcript list", async () => {
        renderHook(() => useTauriEvents());
        await waitForAllHandlers();

        handlers.get("gemini-transcription")?.(
            makeEvent("gemini-transcription", {
                text: "hi there",
                is_final: true,
            }),
        );

        const entries = useAudioGraphStore.getState().geminiTranscripts;
        expect(entries).toHaveLength(1);
        expect(entries[0]).toMatchObject({
            text: "hi there",
            is_final: true,
            source: "gemini",
        });
    });

    it("tolerates partial listen() failures and still cleans up successful listeners", async () => {
        const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
        vi.mocked(listen).mockImplementation(
            async (eventName: string, cb: Handler) => {
                if (eventName === "graph-update") {
                    throw new Error("listen boom");
                }
                handlers.set(eventName, cb);
                const unlisten = vi.fn();
                unlisteners.push(unlisten);
                return unlisten;
            },
        );

        const { unmount } = renderHook(() => useTauriEvents());
        await waitFor(() => {
            // All listeners but the one that was made to throw.
            expect(handlers.size).toBe(TOTAL_LISTENERS - 1);
        });
        expect(handlers.has("graph-update")).toBe(false);

        unmount();
        for (const fn of unlisteners) {
            expect(fn).toHaveBeenCalledTimes(1);
        }
        errSpy.mockRestore();
    });

    it("flips isGeminiActive off when gemini-status 'disconnected' fires", async () => {
        renderHook(() => useTauriEvents());
        await waitForAllHandlers();
        expect(useAudioGraphStore.getState().isGeminiActive).toBe(true);

        handlers.get("gemini-status")?.(
            makeEvent("gemini-status", { type: "disconnected" }),
        );
        expect(useAudioGraphStore.getState().isGeminiActive).toBe(false);
    });

    // ------------------------------------------------------------------
    // ag#13 — AWS error translation
    // ------------------------------------------------------------------
    //
    // The aws-error event is the contract between the backend's
    // UiAwsError taxonomy and the frontend's i18n-backed user messaging.
    // This covers:
    //   1. the category → i18n-key mapping via awsErrorToMessage
    //   2. the listener wiring (payload lands in store.error)
    // Those two together guarantee that a backend event with
    // `category: "invalid_access_key"` surfaces as a localized,
    // actionable message in the global error banner.

    it("translates aws-error payloads via awsErrorToMessage and routes to store.error", async () => {
        const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
        renderHook(() => useTauriEvents());
        await waitForAllHandlers();

        // Invalid access key → check the core mapping is exercised.
        const invalidKey: AwsErrorPayload = {
            error: { category: "invalid_access_key" },
            raw_message: "InvalidClientTokenId: The security token is invalid",
        };
        handlers.get("aws-error")?.(makeEvent("aws-error", invalidKey));
        expect(useAudioGraphStore.getState().error).toContain(
            "Access Key ID not recognized",
        );
        // And the exported helper returns the same string, so future
        // consumers (a diagnostics panel, tests in other modules) stay
        // in sync without duplicating the switch.
        expect(awsErrorToMessage(invalidKey)).toBe(
            useAudioGraphStore.getState().error,
        );

        // Region-parameterised payload renders the region into the message.
        const regionErr: AwsErrorPayload = {
            error: {
                category: "region_not_supported",
                region: "ap-south-2",
            },
            raw_message: "UnrecognizedClientException: wrong region",
        };
        handlers.get("aws-error")?.(makeEvent("aws-error", regionErr));
        expect(useAudioGraphStore.getState().error).toContain("ap-south-2");

        // AccessDenied with a parsed permission surfaces the action name
        // so the user knows which IAM policy is missing.
        const accessDenied: AwsErrorPayload = {
            error: {
                category: "access_denied",
                permission: "transcribe:StartStreamTranscription",
            },
            raw_message: "not authorized to perform: transcribe:StartStreamTranscription",
        };
        handlers.get("aws-error")?.(makeEvent("aws-error", accessDenied));
        expect(useAudioGraphStore.getState().error).toContain(
            "transcribe:StartStreamTranscription",
        );

        errSpy.mockRestore();
    });

    // Exhaustive routing check for every GeminiErrorCategory variant:
    // confirms the (kind → i18n key, toast variant) pairing stays stable.
    // If the category spec ever grows a new variant this test should fail
    // at the switch exhaustiveness check rather than surface the wrong
    // toast severity in production.
    it("routes every Gemini error category to the right i18n key + toast variant", () => {
        const cases: Array<{
            category: GeminiErrorCategory;
            expectedKey: string;
            expectedVariant: "warning" | "info" | "error";
        }> = [
            {
                category: { kind: "auth" },
                expectedKey: "gemini.error.auth",
                expectedVariant: "warning",
            },
            {
                category: { kind: "auth_expired" },
                expectedKey: "gemini.error.authExpired",
                expectedVariant: "warning",
            },
            {
                category: { kind: "rate_limit", retry_after_secs: 30 },
                expectedKey: "gemini.error.rateLimit",
                expectedVariant: "warning",
            },
            {
                category: { kind: "network" },
                expectedKey: "gemini.error.network",
                expectedVariant: "info",
            },
            {
                category: { kind: "server" },
                expectedKey: "gemini.error.server",
                expectedVariant: "error",
            },
            {
                category: { kind: "unknown" },
                expectedKey: "gemini.error.unknown",
                expectedVariant: "error",
            },
        ];

        for (const { category, expectedKey, expectedVariant } of cases) {
            const { key, variant } = routeGeminiError(category);
            expect(key).toBe(expectedKey);
            expect(variant).toBe(expectedVariant);
        }
    });

    it("fires a toast when gemini-status 'error' arrives with a category", async () => {
        vi.mocked(showToast).mockClear();
        renderHook(() => useTauriEvents());
        await waitForAllHandlers();

        handlers.get("gemini-status")?.(
            makeEvent("gemini-status", {
                type: "error",
                message: "WS close 1008 api key invalid",
                category: { kind: "auth" },
            }),
        );

        // Classified errors route through showToast (warning) — they do
        // NOT set the global error banner, because auth failures are
        // recoverable via Settings → Gemini Live.
        expect(vi.mocked(showToast)).toHaveBeenCalledTimes(1);
        const call = vi.mocked(showToast).mock.calls[0][0];
        expect(call.variant).toBe("warning");
    });

    it("falls back to the error banner when gemini-status 'error' has no category", async () => {
        vi.mocked(showToast).mockClear();
        renderHook(() => useTauriEvents());
        await waitForAllHandlers();

        handlers.get("gemini-status")?.(
            makeEvent("gemini-status", {
                type: "error",
                message: "legacy plain-string error",
            }),
        );

        // Legacy events without `category` preserve the prior behavior:
        // the message lands in the banner so existing backend paths keep
        // working during the migration.
        expect(useAudioGraphStore.getState().error).toBe(
            "Gemini: legacy plain-string error",
        );
        expect(vi.mocked(showToast)).not.toHaveBeenCalled();
    });
});
