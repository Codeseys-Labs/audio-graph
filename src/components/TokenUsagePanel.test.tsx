import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { listen } from "@tauri-apps/api/event";
import TokenUsagePanel from "./TokenUsagePanel";
import "../i18n";
import type { GeminiStatusEvent } from "../types";

// The Tauri mock from src/test/setup.ts returns `() => {}` for listen.
// Override it here so we can capture and invoke the handler directly.
type Handler = (event: { payload: GeminiStatusEvent }) => void;

function installListener() {
    const handlers: Handler[] = [];
    const mocked = listen as unknown as ReturnType<typeof vi.fn>;
    mocked.mockImplementation(
        async (_name: string, handler: Handler) => {
            handlers.push(handler);
            return () => {
                const idx = handlers.indexOf(handler);
                if (idx >= 0) handlers.splice(idx, 1);
            };
        },
    );
    return {
        emit(payload: GeminiStatusEvent) {
            for (const h of handlers) h({ payload });
        },
    };
}

async function flushEffects() {
    // Let the async listen() promise resolve + React commit.
    await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
    });
}

describe("TokenUsagePanel", () => {
    beforeEach(() => {
        (listen as unknown as ReturnType<typeof vi.fn>).mockReset();
    });

    it("shows empty state before any usage arrives", () => {
        installListener();
        render(<TokenUsagePanel />);
        expect(
            screen.getByText(/no token usage reported yet/i),
        ).toBeInTheDocument();
    });

    it("accumulates totals across turn_complete events", async () => {
        const bus = installListener();
        render(<TokenUsagePanel />);
        await flushEffects();

        await act(async () => {
            bus.emit({
                type: "turn_complete",
                usage: {
                    promptTokenCount: 100,
                    responseTokenCount: 50,
                    totalTokenCount: 150,
                },
            });
        });
        await act(async () => {
            bus.emit({
                type: "turn_complete",
                usage: {
                    promptTokenCount: 40,
                    responseTokenCount: 10,
                    totalTokenCount: 50,
                    thoughtsTokenCount: 5,
                },
            });
        });

        // Total row reflects sum across both turns (150 + 50 = 200).
        const totalDt = screen.getByText("Total");
        const totalCell = totalDt.parentElement as HTMLElement;
        expect(totalCell).toHaveTextContent("200");

        // Prompt sums to 140.
        const promptCell = screen.getByText("Prompt").parentElement as HTMLElement;
        expect(promptCell).toHaveTextContent("140");

        // Thoughts only showed up on turn 2, sums to 5.
        const thoughtsCell = screen.getByText("Thoughts")
            .parentElement as HTMLElement;
        expect(thoughtsCell).toHaveTextContent("5");
    });

    it("ignores turn_complete events without usage", async () => {
        const bus = installListener();
        render(<TokenUsagePanel />);
        await flushEffects();

        await act(async () => {
            bus.emit({ type: "turn_complete" });
        });

        expect(
            screen.getByText(/no token usage reported yet/i),
        ).toBeInTheDocument();
    });

    it("ignores non-turn_complete status events", async () => {
        const bus = installListener();
        render(<TokenUsagePanel />);
        await flushEffects();

        await act(async () => {
            bus.emit({ type: "connected" });
            bus.emit({
                type: "error",
                message: "boom",
                usage: { promptTokenCount: 999, totalTokenCount: 999 },
            });
        });

        // Error payload with usage is NOT turn_complete, so it must be ignored.
        expect(
            screen.getByText(/no token usage reported yet/i),
        ).toBeInTheDocument();
    });

    it("reset clears accumulated totals", async () => {
        const bus = installListener();
        render(<TokenUsagePanel />);
        await flushEffects();

        await act(async () => {
            bus.emit({
                type: "turn_complete",
                usage: { totalTokenCount: 123, promptTokenCount: 100 },
            });
        });

        const totalCell = screen.getByText("Total").parentElement as HTMLElement;
        expect(totalCell).toHaveTextContent("123");

        await userEvent.click(screen.getByRole("button", { name: /reset/i }));

        expect(
            screen.getByText(/no token usage reported yet/i),
        ).toBeInTheDocument();
    });
});
