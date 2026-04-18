import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import type { GeminiStatusEvent, UsageMetadata } from "../types";

const GEMINI_STATUS = "gemini-status";

interface Totals {
    prompt: number;
    response: number;
    cached: number;
    thoughts: number;
    toolUse: number;
    total: number;
    turns: number;
}

const ZERO_TOTALS: Totals = {
    prompt: 0,
    response: 0,
    cached: 0,
    thoughts: 0,
    toolUse: 0,
    total: 0,
    turns: 0,
};

function add(totals: Totals, u: UsageMetadata): Totals {
    return {
        prompt: totals.prompt + (u.promptTokenCount ?? 0),
        response: totals.response + (u.responseTokenCount ?? 0),
        cached: totals.cached + (u.cachedContentTokenCount ?? 0),
        thoughts: totals.thoughts + (u.thoughtsTokenCount ?? 0),
        toolUse: totals.toolUse + (u.toolUsePromptTokenCount ?? 0),
        total: totals.total + (u.totalTokenCount ?? 0),
        turns: totals.turns + 1,
    };
}

function formatCount(n: number): string {
    return n.toLocaleString();
}

function TokenUsagePanel() {
    const { t } = useTranslation();
    const [totals, setTotals] = useState<Totals>(ZERO_TOTALS);
    const [lastUsage, setLastUsage] = useState<UsageMetadata | null>(null);

    useEffect(() => {
        let unlisten: (() => void) | null = null;
        let cancelled = false;

        (async () => {
            const off = await listen<GeminiStatusEvent>(GEMINI_STATUS, (event) => {
                const payload = event.payload;
                if (payload.type !== "turn_complete" || !payload.usage) return;
                setTotals((prev) => add(prev, payload.usage as UsageMetadata));
                setLastUsage(payload.usage);
            });
            if (cancelled) {
                off();
            } else {
                unlisten = off;
            }
        })();

        return () => {
            cancelled = true;
            if (unlisten) unlisten();
        };
    }, []);

    const handleReset = useCallback(() => {
        setTotals(ZERO_TOTALS);
        setLastUsage(null);
    }, []);

    const hasData = totals.turns > 0;

    return (
        <section
            className="token-usage"
            aria-label={t("tokens.title")}
        >
            <div className="token-usage__header">
                <h3 className="panel-title">{t("tokens.title")}</h3>
                <div className="token-usage__header-actions">
                    {hasData && (
                        <span
                            className="token-usage__turns"
                            title={t("tokens.turnsTooltip")}
                        >
                            {t("tokens.turns", { count: totals.turns })}
                        </span>
                    )}
                    <button
                        type="button"
                        className="panel-export-btn"
                        onClick={handleReset}
                        disabled={!hasData}
                        aria-label={t("tokens.reset")}
                        title={t("tokens.reset")}
                    >
                        {t("tokens.reset")}
                    </button>
                </div>
            </div>

            {!hasData ? (
                <p className="token-usage__empty">{t("tokens.empty")}</p>
            ) : (
                <dl className="token-usage__grid">
                    <div className="token-usage__cell token-usage__cell--total">
                        <dt>{t("tokens.total")}</dt>
                        <dd>{formatCount(totals.total)}</dd>
                    </div>
                    <div className="token-usage__cell">
                        <dt>{t("tokens.prompt")}</dt>
                        <dd>{formatCount(totals.prompt)}</dd>
                    </div>
                    <div className="token-usage__cell">
                        <dt>{t("tokens.response")}</dt>
                        <dd>{formatCount(totals.response)}</dd>
                    </div>
                    {totals.thoughts > 0 && (
                        <div className="token-usage__cell">
                            <dt>{t("tokens.thoughts")}</dt>
                            <dd>{formatCount(totals.thoughts)}</dd>
                        </div>
                    )}
                    {totals.toolUse > 0 && (
                        <div className="token-usage__cell">
                            <dt>{t("tokens.toolUse")}</dt>
                            <dd>{formatCount(totals.toolUse)}</dd>
                        </div>
                    )}
                    {totals.cached > 0 && (
                        <div className="token-usage__cell">
                            <dt>{t("tokens.cached")}</dt>
                            <dd>{formatCount(totals.cached)}</dd>
                        </div>
                    )}
                </dl>
            )}

            {lastUsage && (
                <p
                    className="token-usage__last"
                    title={t("tokens.lastTurnTooltip")}
                >
                    {t("tokens.lastTurn", {
                        total: formatCount(lastUsage.totalTokenCount ?? 0),
                    })}
                </p>
            )}
        </section>
    );
}

export default TokenUsagePanel;
