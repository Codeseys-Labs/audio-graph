import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  RUNTIME_DIAGNOSTIC_CONTRACT,
  type RuntimeDiagnostic,
} from "../generated/runtimeDiagnostic";
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import type { ChatMessage } from "../types";
import ChatSidebar from "./ChatSidebar";

// jsdom implements neither scrollIntoView nor matchMedia, both reached by the
// auto-scroll effect (scrollBehavior -> matchMedia). Stub them so the effect
// runs without throwing.
beforeEach(async () => {
  await i18n.changeLanguage("en");
  HTMLElement.prototype.scrollIntoView = vi.fn();
  if (!window.matchMedia) {
    window.matchMedia = vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    }));
  }
});

function resetStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    chatMessages: [],
    isChatLoading: false,
    sendChatMessage: vi.fn(async () => {}),
    clearChatHistory: vi.fn(async () => {}),
    graphSnapshot: {
      nodes: [],
      links: [],
      stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
    },
    loadedSessionId: null,
    ...overrides,
  });
}

function msg(role: ChatMessage["role"], content: string): ChatMessage {
  return { role, content };
}

function runtimeDiagnostic(
  code: (typeof RUNTIME_DIAGNOSTIC_CONTRACT.error_codes)[number],
  recoveryAction: (typeof RUNTIME_DIAGNOSTIC_CONTRACT.recovery_actions)[number] = "none",
): RuntimeDiagnostic {
  return {
    kind: "runtime",
    detail: {
      code,
      retryable: false,
      recovery_action: recoveryAction,
      context: { operation: "language_model_inference" },
    },
  };
}

function renderFailedTurn(failure: RuntimeDiagnostic, content = "") {
  resetStore({
    chatMessages: [msg("user", "q"), { role: "assistant", content, failure }],
  });
  render(<ChatSidebar />);
  return screen.getByRole("alert");
}

describe("ChatSidebar", () => {
  beforeEach(() => {
    resetStore();
  });

  it("renders the header with an aria-live message log", () => {
    render(<ChatSidebar />);
    expect(screen.getByRole("heading", { name: /chat/i })).toBeInTheDocument();
    const log = screen.getByRole("log", { name: /chat messages/i });
    expect(log).toHaveAttribute("aria-live", "polite");
  });

  it("shows the empty state with example prompts when there are no messages", () => {
    render(<ChatSidebar />);
    expect(
      screen.getByText(/ask questions about the conversation/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/summarize the conversation so far/i),
    ).toBeInTheDocument();
  });

  it("shows the graph-context entity count from the snapshot stats", () => {
    resetStore({
      graphSnapshot: {
        nodes: [],
        links: [],
        stats: { total_nodes: 12, total_edges: 3, total_episodes: 0 },
      },
    });
    render(<ChatSidebar />);
    expect(screen.getByText(/12 entities/i)).toBeInTheDocument();
  });

  it("renders the message list with role labels and content", () => {
    resetStore({
      chatMessages: [
        msg("user", "What entities are there?"),
        msg("assistant", "Alice and Bob."),
      ],
    });
    render(<ChatSidebar />);
    expect(screen.getByText("What entities are there?")).toBeInTheDocument();
    expect(screen.getByText("Alice and Bob.")).toBeInTheDocument();
    expect(screen.getByText("You")).toBeInTheDocument();
    expect(screen.getByText("Assistant")).toBeInTheDocument();
    // Empty-state copy is gone once messages exist.
    expect(
      screen.queryByText(/ask questions about the conversation/i),
    ).not.toBeInTheDocument();
  });

  it("hides the clear button when there are no messages", () => {
    render(<ChatSidebar />);
    expect(
      screen.queryByRole("button", { name: /clear chat history/i }),
    ).not.toBeInTheDocument();
  });

  it("shows the clear button once there is at least one message", () => {
    resetStore({ chatMessages: [msg("user", "hi")] });
    render(<ChatSidebar />);
    expect(
      screen.getByRole("button", { name: /clear chat history/i }),
    ).toBeInTheDocument();
  });

  it("clicking clear calls clearChatHistory", () => {
    const clearChatHistory = vi.fn(async () => {});
    resetStore({ chatMessages: [msg("user", "hi")], clearChatHistory });
    render(<ChatSidebar />);
    fireEvent.click(
      screen.getByRole("button", { name: /clear chat history/i }),
    );
    expect(clearChatHistory).toHaveBeenCalledTimes(1);
  });

  it("shows the streaming 'thinking' indicator while loading", () => {
    resetStore({ isChatLoading: true });
    render(<ChatSidebar />);
    expect(screen.getByText(/assistant is thinking/i)).toBeInTheDocument();
    // The empty-state copy must NOT show while a request is in flight.
    expect(
      screen.queryByText(/ask questions about the conversation/i),
    ).not.toBeInTheDocument();
  });

  it("send button is disabled with empty input and enabled once text is typed", () => {
    render(<ChatSidebar />);
    const send = screen.getByRole("button", { name: /send message/i });
    expect(send).toBeDisabled();
    const input = screen.getByRole("textbox", {
      name: /ask about the conversation/i,
    });
    fireEvent.change(input, { target: { value: "hello" } });
    expect(send).toBeEnabled();
  });

  it("send button stays disabled for whitespace-only input", () => {
    render(<ChatSidebar />);
    const input = screen.getByRole("textbox", {
      name: /ask about the conversation/i,
    });
    fireEvent.change(input, { target: { value: "   " } });
    expect(
      screen.getByRole("button", { name: /send message/i }),
    ).toBeDisabled();
  });

  it("clicking send dispatches the trimmed message and clears the input", async () => {
    const sendChatMessage = vi.fn(async () => {});
    resetStore({ sendChatMessage });
    render(<ChatSidebar />);
    const input = screen.getByRole("textbox", {
      name: /ask about the conversation/i,
    }) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "  summarize  " } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /send message/i }));
    });
    await waitFor(() =>
      expect(sendChatMessage).toHaveBeenCalledWith("summarize"),
    );
    expect(input.value).toBe("");
  });

  it("pressing Enter sends; Shift+Enter does not", async () => {
    const sendChatMessage = vi.fn(async () => {});
    resetStore({ sendChatMessage });
    render(<ChatSidebar />);
    const input = screen.getByRole("textbox", {
      name: /ask about the conversation/i,
    });
    fireEvent.change(input, { target: { value: "question" } });

    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(sendChatMessage).not.toHaveBeenCalled();

    await act(async () => {
      fireEvent.keyDown(input, { key: "Enter" });
    });
    await waitFor(() =>
      expect(sendChatMessage).toHaveBeenCalledWith("question"),
    );
  });

  it("does not send while a request is loading", () => {
    const sendChatMessage = vi.fn(async () => {});
    resetStore({ sendChatMessage, isChatLoading: true });
    render(<ChatSidebar />);
    const input = screen.getByRole("textbox", {
      name: /ask about the conversation/i,
    });
    // Input is disabled while loading; drive handleSend via Enter regardless.
    fireEvent.keyDown(input, { key: "Enter" });
    expect(sendChatMessage).not.toHaveBeenCalled();
  });

  it("blocks sending to the live backend while reviewing a historical session", () => {
    const sendChatMessage = vi.fn(async () => {});
    resetStore({ sendChatMessage, loadedSessionId: "historical-session" });
    render(<ChatSidebar />);

    const input = screen.getByRole("textbox", {
      name: /ask about the conversation/i,
    });
    expect(input).toBeDisabled();
    expect(input).toHaveAttribute("aria-describedby", "chat-review-send-help");
    expect(screen.getByRole("status")).toHaveTextContent(
      /reviewing a past session.*live workspace/i,
    );

    fireEvent.change(input, { target: { value: "summarize" } });
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.click(screen.getByRole("button", { name: /send message/i }));
    expect(sendChatMessage).not.toHaveBeenCalled();
  });

  it("localizes the historical-review chat guard", async () => {
    await i18n.changeLanguage("pt");
    resetStore({ loadedSessionId: "sessao-historica" });
    render(<ChatSidebar />);

    expect(screen.getByRole("status")).toHaveTextContent(
      /revisão de uma sessão anterior.*espaço de trabalho ao vivo/i,
    );
    expect(
      screen.getByRole("textbox", { name: /pergunte sobre a conversa/i }),
    ).toBeDisabled();
  });

  describe("failed turns", () => {
    it.each(
      RUNTIME_DIAGNOSTIC_CONTRACT.error_codes,
    )("renders localized copy for %s, never the raw token", (code) => {
      const alert = renderFailedTurn(runtimeDiagnostic(code));
      expect(alert).toHaveTextContent(i18n.t("chat.failed"));
      expect(alert).toHaveTextContent(
        i18n.t(`runtimeDiagnostics.runtime.${code}`),
      );
      expect(alert.textContent).not.toContain("runtime/");
      // The rate-limit guidance is keyed on the code, not on text.
      if (code === "rate_limited") {
        expect(alert).toHaveTextContent(i18n.t("chat.rateLimitedHint"));
      } else {
        expect(alert).not.toHaveTextContent(i18n.t("chat.rateLimitedHint"));
      }
    });

    it("shows the recovery step and keeps partial text visible but labeled", () => {
      const alert = renderFailedTurn(
        runtimeDiagnostic("credential_rejected", "reenter_credential"),
        "Half an answer",
      );
      expect(alert).toHaveTextContent("Re-enter the credential in Settings.");
      expect(screen.getByText("Half an answer")).toBeInTheDocument();
      expect(screen.getByText(i18n.t("chat.partialReply"))).toBeInTheDocument();
    });

    it("fails closed to the fixed internal copy for a malformed diagnostic", () => {
      const alert = renderFailedTurn({
        kind: "runtime",
        detail: { code: "runtime/provider_unavailable" },
      } as unknown as RuntimeDiagnostic);
      expect(alert).toHaveTextContent(
        i18n.t("runtimeDiagnostics.runtime.internal"),
      );
      expect(alert.textContent).not.toContain("runtime/");
    });

    it("localizes the failure copy", async () => {
      await act(async () => {
        await i18n.changeLanguage("pt");
      });
      const alert = renderFailedTurn(runtimeDiagnostic("rate_limited"));
      expect(alert).toHaveTextContent("A resposta falhou.");
      expect(alert).toHaveTextContent(
        "O provedor está limitando as solicitações. Tente novamente mais tarde.",
      );
    });
  });
});
