import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { ChatMessage } from "../types";
import ChatSidebar from "./ChatSidebar";

// jsdom implements neither scrollIntoView nor matchMedia, both reached by the
// auto-scroll effect (scrollBehavior -> matchMedia). Stub them so the effect
// runs without throwing.
beforeEach(() => {
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
    ...overrides,
  });
}

function msg(role: ChatMessage["role"], content: string): ChatMessage {
  return { role, content };
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
});
