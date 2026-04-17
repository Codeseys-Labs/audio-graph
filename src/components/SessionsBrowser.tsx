import { useEffect } from "react";
import { useAudioGraphStore } from "../store";
import type { SessionMetadata } from "../types";

/** Format a unix-millis timestamp into a short, human-readable local string. */
function formatTimestamp(ms: number): string {
    if (!ms) return "—";
    return new Date(ms).toLocaleString();
}

/** Format a duration in seconds as "Hh Mm" or "Mm Ss". */
function formatDuration(seconds: number | null): string {
    if (seconds === null || seconds === undefined) return "—";
    if (seconds < 60) return `${seconds}s`;
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = seconds % 60;
    if (h > 0) return `${h}h ${m}m`;
    return `${m}m ${s}s`;
}

/** CSS-class-friendly modifier for a session's status. */
function statusModifier(status: SessionMetadata["status"]): string {
    return `sessions-browser__status--${status}`;
}

function SessionsBrowser() {
    const sessions = useAudioGraphStore((s) => s.sessions);
    const sessionsLoading = useAudioGraphStore((s) => s.sessionsLoading);
    const listSessions = useAudioGraphStore((s) => s.listSessions);
    const loadSessionTranscript = useAudioGraphStore(
        (s) => s.loadSessionTranscript,
    );
    const deleteSession = useAudioGraphStore((s) => s.deleteSession);
    const closeSessionsBrowser = useAudioGraphStore(
        (s) => s.closeSessionsBrowser,
    );
    const setRightPanelTab = useAudioGraphStore((s) => s.setRightPanelTab);

    // Refresh on mount (store already kicks off a fetch when opening, but
    // calling again here is cheap and ensures the list is fresh).
    useEffect(() => {
        void listSessions(10);
    }, [listSessions]);

    const handleLoad = async (sessionId: string) => {
        await loadSessionTranscript(sessionId);
        // Make sure the transcript tab is visible so the user can see the result.
        setRightPanelTab("transcript");
        closeSessionsBrowser();
    };

    const handleDelete = async (sessionId: string) => {
        // Minimal v1: plain confirm. No custom confirmation modal.
        const ok = window.confirm(
            "Delete this session? The transcript and graph files will be removed from disk.",
        );
        if (!ok) return;
        await deleteSession(sessionId);
    };

    return (
        <div
            className="settings-overlay"
            onClick={closeSessionsBrowser}
            role="dialog"
            aria-modal="true"
            aria-label="Sessions browser"
        >
            <div
                className="settings-modal sessions-browser"
                onClick={(e) => e.stopPropagation()}
            >
                <div className="settings-header">
                    <h2 className="settings-header__title">Recent Sessions</h2>
                    <button
                        className="settings-header__close"
                        onClick={closeSessionsBrowser}
                        aria-label="Close sessions browser"
                    >
                        ✕
                    </button>
                </div>

                <div className="settings-content">
                    {sessionsLoading ? (
                        <p>Loading sessions…</p>
                    ) : sessions.length === 0 ? (
                        <p className="settings-section__empty">
                            No sessions yet. Start a capture to create one.
                        </p>
                    ) : (
                        <ul
                            className="sessions-browser__list"
                            style={{
                                listStyle: "none",
                                padding: 0,
                                margin: 0,
                                display: "flex",
                                flexDirection: "column",
                                gap: "8px",
                            }}
                        >
                            {sessions.map((s) => (
                                <li
                                    key={s.id}
                                    className="sessions-browser__item"
                                    style={{
                                        border: "1px solid var(--border, #333)",
                                        borderRadius: "6px",
                                        padding: "10px 12px",
                                        display: "flex",
                                        flexDirection: "column",
                                        gap: "6px",
                                    }}
                                >
                                    <div
                                        style={{
                                            display: "flex",
                                            justifyContent: "space-between",
                                            alignItems: "baseline",
                                            gap: "8px",
                                        }}
                                    >
                                        <div
                                            style={{
                                                display: "flex",
                                                flexDirection: "column",
                                                gap: "2px",
                                                minWidth: 0,
                                            }}
                                        >
                                            <strong
                                                style={{
                                                    fontSize: "0.95em",
                                                    overflow: "hidden",
                                                    textOverflow: "ellipsis",
                                                    whiteSpace: "nowrap",
                                                }}
                                                title={s.id}
                                            >
                                                {s.title ?? s.id.slice(0, 8)}
                                            </strong>
                                            <span
                                                style={{
                                                    fontSize: "0.8em",
                                                    opacity: 0.7,
                                                }}
                                            >
                                                {formatTimestamp(s.created_at)}
                                            </span>
                                        </div>
                                        <span
                                            className={`sessions-browser__status ${statusModifier(s.status)}`}
                                            style={{
                                                fontSize: "0.75em",
                                                padding: "2px 8px",
                                                borderRadius: "999px",
                                                border: "1px solid currentColor",
                                                opacity: 0.8,
                                                textTransform: "capitalize",
                                                whiteSpace: "nowrap",
                                            }}
                                        >
                                            {s.status}
                                        </span>
                                    </div>

                                    <div
                                        style={{
                                            fontSize: "0.8em",
                                            opacity: 0.75,
                                            display: "flex",
                                            gap: "12px",
                                            flexWrap: "wrap",
                                        }}
                                    >
                                        <span>
                                            Duration: {formatDuration(s.duration_seconds)}
                                        </span>
                                        <span>Segments: {s.segment_count}</span>
                                        <span>Speakers: {s.speaker_count}</span>
                                        <span>Entities: {s.entity_count}</span>
                                    </div>

                                    <div
                                        style={{
                                            display: "flex",
                                            gap: "8px",
                                            justifyContent: "flex-end",
                                        }}
                                    >
                                        <button
                                            className="settings-btn settings-btn--primary"
                                            onClick={() => handleLoad(s.id)}
                                            title="Load this session's transcript into the view"
                                        >
                                            Load
                                        </button>
                                        <button
                                            className="settings-btn settings-btn--danger"
                                            onClick={() => handleDelete(s.id)}
                                            title="Delete this session and its files"
                                        >
                                            Delete
                                        </button>
                                    </div>
                                </li>
                            ))}
                        </ul>
                    )}
                </div>
            </div>
        </div>
    );
}

export default SessionsBrowser;
