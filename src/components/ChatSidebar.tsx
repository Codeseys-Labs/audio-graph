/**
 * Chat sidebar — free-form chat turns grounded in the current knowledge
 * graph.
 *
 * The user types a prompt; the backend `send_chat_message` command injects
 * the latest `GraphSnapshot` as context so the LLM (local llama/mistralrs
 * or an OpenAI-compatible API) can reason over extracted entities and
 * relations. Auto-scrolls to the bottom on new messages.
 *
 * Store bindings: `chatMessages`, `isChatLoading`, `sendChatMessage`,
 * `clearChatHistory`, `graphSnapshot`.
 *
 * Parent: `App.tsx` right-panel tab. No props — rendered only when the
 * `rightPanelTab` store slice equals `"chat"`.
 */
import { useState, useRef, useEffect } from "react";
import { useAudioGraphStore } from "../store";
import type { ChatMessage } from "../types";
import { scrollBehavior } from "../utils/motion";
import Icon from "./Icon";
import IconButton from "./IconButton";

function ChatSidebar() {
    const chatMessages = useAudioGraphStore((s) => s.chatMessages);
    const isChatLoading = useAudioGraphStore((s) => s.isChatLoading);
    const sendChatMessage = useAudioGraphStore((s) => s.sendChatMessage);
    const clearChatHistory = useAudioGraphStore((s) => s.clearChatHistory);
    const graphSnapshot = useAudioGraphStore((s) => s.graphSnapshot);

    const [input, setInput] = useState("");
    const messagesEndRef = useRef<HTMLDivElement>(null);
    const inputRef = useRef<HTMLInputElement>(null);

    // Auto-scroll to bottom on new messages (respects OS reduced-motion).
    useEffect(() => {
        messagesEndRef.current?.scrollIntoView({ behavior: scrollBehavior() });
    }, [chatMessages, isChatLoading]);

    const handleSend = async () => {
        const trimmed = input.trim();
        if (!trimmed || isChatLoading) return;
        setInput("");
        await sendChatMessage(trimmed);
        inputRef.current?.focus();
    };

    const handleKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            handleSend();
        }
    };

    // Message bubble content shares a base; user/assistant differ in surface,
    // text colour, and the squared-off corner (preserves the original
    // `.chat-sidebar__message--{role} .chat-sidebar__message-content` rules).
    const messageContentBase =
        "py-(--space-4) px-(--space-5) rounded-xl text-[0.85rem] leading-[1.4] whitespace-pre-wrap break-words";

    return (
        <div className="flex flex-col h-full overflow-hidden">
            <div className="flex items-center justify-between py-[10px] px-(--space-5) border-b border-border-color shrink-0">
                <h3 className="m-0 text-[0.95rem] font-semibold text-text-primary"><Icon name="chat" size={16} /> Chat</h3>
                <div className="flex items-center gap-(--space-4)">
                    <span className="text-[0.7rem] py-(--space-1) px-(--space-3) rounded-lg bg-[rgba(96,165,250,0.15)] text-accent-blue" title="Graph context available">
                        {graphSnapshot.stats.total_nodes} entities
                    </span>
                    {chatMessages.length > 0 && (
                        <IconButton
                            className="bg-none border-none cursor-pointer text-[0.85rem] py-(--space-1) px-(--space-2) rounded-sm opacity-60 transition-[opacity] duration-200 hover:opacity-100 hover:bg-[rgba(255,255,255,0.05)]"
                            icon="trash"
                            label="Clear chat history"
                            variant="ghost"
                            onClick={clearChatHistory}
                        />
                    )}
                </div>
            </div>

            <div
                className="flex-1 overflow-y-auto p-(--space-5) flex flex-col gap-[10px]"
                role="log"
                aria-live="polite"
                aria-label="Chat messages"
            >
                {chatMessages.length === 0 && !isChatLoading && (
                    <div className="flex flex-col items-center justify-center h-full text-center text-text-secondary p-(--space-7)">
                        <p className="my-(--space-2) text-[0.85rem]">Ask questions about the conversation and knowledge graph.</p>
                        <p className="text-[0.75rem]! text-text-muted! italic mt-(--space-4)!">
                            Try: "What entities have been mentioned?" or "Summarize the conversation so far"
                        </p>
                    </div>
                )}

                {chatMessages.map((msg: ChatMessage, idx: number) => (
                    <div
                        key={`${msg.role}-${idx}`}
                        className={`flex flex-col max-w-[90%] ${msg.role === "user" ? "self-end" : "self-start"}`}
                    >
                        <div className="text-[0.65rem] uppercase tracking-[0.05em] text-text-muted mb-(--space-1) px-(--space-2)">
                            {msg.role === "user" ? "You" : "Assistant"}
                        </div>
                        <div
                            className={
                                msg.role === "user"
                                    ? `${messageContentBase} bg-accent-blue text-(--on-accent-blue) rounded-br-sm`
                                    : `${messageContentBase} bg-[rgba(255,255,255,0.06)] text-text-primary border border-border-color rounded-bl-sm`
                            }
                        >
                            {msg.content}
                        </div>
                    </div>
                ))}

                {isChatLoading && (
                    <div className="flex flex-col max-w-[90%] self-start">
                        <div className="text-[0.65rem] uppercase tracking-[0.05em] text-text-muted mb-(--space-1) px-(--space-2)">Assistant</div>
                        <div className="flex gap-(--space-2) py-(--space-4) px-(--space-5) bg-[rgba(255,255,255,0.06)] border border-border-color rounded-xl rounded-bl-sm" role="status">
                            <span className="sr-only">Assistant is thinking…</span>
                            <span className="w-[6px] h-[6px] rounded-full bg-text-secondary animate-[chat-dot-bounce_1.4s_infinite_ease-in-out_both] [animation-delay:-0.32s]" aria-hidden="true"></span>
                            <span className="w-[6px] h-[6px] rounded-full bg-text-secondary animate-[chat-dot-bounce_1.4s_infinite_ease-in-out_both] [animation-delay:-0.16s]" aria-hidden="true"></span>
                            <span className="w-[6px] h-[6px] rounded-full bg-text-secondary animate-[chat-dot-bounce_1.4s_infinite_ease-in-out_both] [animation-delay:0s]" aria-hidden="true"></span>
                        </div>
                    </div>
                )}

                <div ref={messagesEndRef} />
            </div>

            <div className="flex gap-(--space-3) py-[10px] px-(--space-5) border-t border-border-color bg-bg-secondary shrink-0">
                <input
                    ref={inputRef}
                    type="text"
                    className="flex-1 py-(--space-4) px-(--space-5) border border-border-color rounded-lg bg-[rgba(255,255,255,0.04)] text-text-primary text-[0.85rem] outline-none transition-[border-color] duration-200 focus:border-accent-blue placeholder:text-text-muted disabled:opacity-50"
                    placeholder="Ask about the conversation..."
                    aria-label="Ask about the conversation"
                    value={input}
                    onChange={(e) => setInput(e.target.value)}
                    onKeyDown={handleKeyDown}
                    disabled={isChatLoading}
                />
                <IconButton
                    className="py-(--space-4) px-[14px] border-none rounded-lg bg-accent-blue text-(--on-accent-blue) text-[1rem] cursor-pointer transition-all duration-200 shrink-0 hover:not-disabled:bg-(--accent-blue-hover) hover:not-disabled:scale-105 disabled:opacity-40 disabled:cursor-not-allowed"
                    icon="send"
                    label="Send message"
                    onClick={handleSend}
                    disabled={!input.trim() || isChatLoading}
                />
            </div>
        </div>
    );
}

export default ChatSidebar;
