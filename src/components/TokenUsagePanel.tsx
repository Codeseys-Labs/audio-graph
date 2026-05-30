/**
 * Token usage panel — shows Gemini Live token totals for the current
 * session alongside a lifetime accumulator.
 *
 * The authoritative on-disk record lives at
 * `~/.audiograph/usage/<session_id>.json` and is written by the backend's
 * `TurnComplete` handler (see `src-tauri/src/sessions/usage.rs`). This
 * component mirrors that store in the frontend by (a) seeding from
 * `localStorage` (`tokens.session.v1` / `tokens.lifetime.v1`) for fast
 * first-paint, (b) re-fetching via `get_current_session_usage` /
 * `get_lifetime_usage` commands on mount, and (c) incrementally applying
 * `UsageMetadata` frames delivered on `GEMINI_STATUS` events.
 *
 * Parent: `App.tsx` right panel (bottom, always visible beneath the tab
 * content). No props.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  GeminiStatusEvent,
  LifetimeUsage,
  SessionUsage,
  UsageMetadata,
} from "../types";

const GEMINI_STATUS = "gemini-status";
const SESSION_KEY = "tokens.session.v1";
const LIFETIME_KEY = "tokens.lifetime.v1";

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

function isFiniteNumber(v: unknown): v is number {
  return typeof v === "number" && Number.isFinite(v);
}

function parseTotals(raw: string | null): Totals {
  if (!raw) return ZERO_TOTALS;
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return ZERO_TOTALS;
    const p = parsed as Record<string, unknown>;
    const out: Totals = {
      prompt: isFiniteNumber(p.prompt) ? p.prompt : 0,
      response: isFiniteNumber(p.response) ? p.response : 0,
      cached: isFiniteNumber(p.cached) ? p.cached : 0,
      thoughts: isFiniteNumber(p.thoughts) ? p.thoughts : 0,
      toolUse: isFiniteNumber(p.toolUse) ? p.toolUse : 0,
      total: isFiniteNumber(p.total) ? p.total : 0,
      turns: isFiniteNumber(p.turns) ? p.turns : 0,
    };
    return out;
  } catch {
    return ZERO_TOTALS;
  }
}

function loadTotals(key: string): Totals {
  if (typeof window === "undefined" || !window.localStorage) return ZERO_TOTALS;
  try {
    return parseTotals(window.localStorage.getItem(key));
  } catch {
    return ZERO_TOTALS;
  }
}

function saveTotals(key: string, totals: Totals): void {
  if (typeof window === "undefined" || !window.localStorage) return;
  try {
    window.localStorage.setItem(key, JSON.stringify(totals));
  } catch {
    // Storage full or denied — silently ignore; in-memory state still works.
  }
}

function removeKey(key: string): void {
  if (typeof window === "undefined" || !window.localStorage) return;
  try {
    window.localStorage.removeItem(key);
  } catch {
    // ignore
  }
}

function sessionUsageToTotals(u: SessionUsage): Totals {
  return {
    prompt: u.prompt,
    response: u.response,
    cached: u.cached,
    thoughts: u.thoughts,
    toolUse: u.tool_use,
    total: u.total,
    turns: u.turns,
  };
}

function lifetimeUsageToTotals(u: LifetimeUsage): Totals {
  return {
    prompt: u.prompt,
    response: u.response,
    cached: u.cached,
    thoughts: u.thoughts,
    toolUse: u.tool_use,
    total: u.total,
    turns: u.turns,
  };
}

/**
 * Build the backend-shaped migration payload from the frontend's
 * localStorage `Totals`. Field renames: `toolUse` → `tool_use`; `sessions`
 * is a backend-only aggregate count and has no localStorage source so we
 * pass 0. The backend's seed function treats this record as a single
 * synthetic session file.
 */
function totalsToLifetimeUsage(t: Totals): LifetimeUsage {
  return {
    prompt: t.prompt,
    response: t.response,
    cached: t.cached,
    thoughts: t.thoughts,
    tool_use: t.toolUse,
    total: t.total,
    turns: t.turns,
    sessions: 0,
  };
}

/**
 * One-shot migration: if the frontend's `localStorage` holds pre-backend
 * lifetime totals, hand them to the backend so `get_lifetime_usage` can
 * include them in its aggregate. Always clears the localStorage key at
 * the end so a re-mount or a reload is a no-op.
 *
 * The flow is intentionally conservative:
 *   1. Parse the `localStorage` value. If it's zero/missing/malformed,
 *      just clear the key and return — nothing to migrate.
 *   2. Probe the backend's current `get_lifetime_usage`. If it already
 *      reports usage (total > 0), the user has migrated on a prior run
 *      (or has real usage from before the localStorage was written). In
 *      either case we do NOT want to seed again — the backend is the
 *      authoritative source, and the backend-side idempotency guard would
 *      reject anyway. Clear localStorage and return.
 *   3. Backend reports zero → seed with the localStorage totals. The
 *      backend will write a `migration-<ms>.json` file that subsequent
 *      `get_lifetime_usage` calls will fold into the sum.
 *
 * Errors from any Tauri call are swallowed here: this runs in dev-mode
 * browsers (no Tauri), on the backend's first install, and during normal
 * operation. None of those cases should surface a toast or block the UI.
 */
async function migrateLocalStorageLifetime(): Promise<void> {
  if (typeof window === "undefined" || !window.localStorage) return;
  const raw = window.localStorage.getItem(LIFETIME_KEY);
  if (!raw) return;
  const parsed = parseTotals(raw);
  // Zero totals: nothing to migrate. Still clear the key so we don't
  // repeat this probe on every mount.
  if (parsed.total <= 0 && parsed.turns <= 0) {
    removeKey(LIFETIME_KEY);
    return;
  }
  try {
    const backend = await invoke<LifetimeUsage>("get_lifetime_usage");
    if (backend.total > 0 || backend.turns > 0) {
      // Backend already has data — don't double-count. Drop the
      // localStorage copy; it's superseded.
      removeKey(LIFETIME_KEY);
      return;
    }
    await invoke("seed_lifetime_migration", {
      payload: totalsToLifetimeUsage(parsed),
    });
  } catch {
    // Tauri unavailable (dev in browser) or backend errored. Leave the
    // localStorage key in place so a future Tauri-enabled mount can
    // retry the migration — clearing would lose the data forever.
    return;
  }
  // Successful seed: drop the localStorage copy so a re-mount doesn't
  // retrigger the probe. The backend's idempotency guard would reject
  // anyway, but clearing keeps the mount path cheap.
  removeKey(LIFETIME_KEY);
}

function TokenUsagePanel() {
  const { t } = useTranslation();
  // Initial state hydrates from localStorage so the cells never flash empty
  // during the async backend round-trip. Backend values overwrite this once
  // the mount-effect `invoke`s resolve.
  const [session, setSession] = useState<Totals>(() => loadTotals(SESSION_KEY));
  const [lifetime, setLifetime] = useState<Totals>(() =>
    loadTotals(LIFETIME_KEY),
  );
  const [lastUsage, setLastUsage] = useState<UsageMetadata | null>(null);

  // Backend hydration. Happens once on mount. If the Tauri command fails
  // (e.g. during dev in a browser without Tauri, or if the backend isn't
  // ready), we keep whatever localStorage gave us — see the lazy initializers
  // above. The Promise.allSettled ensures a single command failing doesn't
  // stall the other.
  //
  // Before hydration, run a one-shot localStorage → backend migration so
  // any lifetime usage that accrued before the backend persistence shipped
  // (key `tokens.lifetime.v1`) feeds into `get_lifetime_usage`. The
  // migration is idempotent on both sides: the backend refuses to write
  // a second migration file, and we clear the localStorage key after a
  // successful probe so a re-mount doesn't keep re-sending the same bytes.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      await migrateLocalStorageLifetime();
      if (cancelled) return;
      const [sessionResult, lifetimeResult] = await Promise.allSettled([
        invoke<SessionUsage>("get_current_session_usage"),
        invoke<LifetimeUsage>("get_lifetime_usage"),
      ]);
      if (cancelled) return;
      if (sessionResult.status === "fulfilled") {
        const next = sessionUsageToTotals(sessionResult.value);
        setSession(next);
        saveTotals(SESSION_KEY, next);
      }
      if (lifetimeResult.status === "fulfilled") {
        const next = lifetimeUsageToTotals(lifetimeResult.value);
        setLifetime(next);
        saveTotals(LIFETIME_KEY, next);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;

    (async () => {
      const off = await listen<GeminiStatusEvent>(GEMINI_STATUS, (event) => {
        const payload = event.payload;
        if (payload.type !== "turn_complete" || !payload.usage) return;
        const usage = payload.usage as UsageMetadata;
        setSession((prev) => {
          const next = add(prev, usage);
          saveTotals(SESSION_KEY, next);
          return next;
        });
        setLifetime((prev) => {
          const next = add(prev, usage);
          saveTotals(LIFETIME_KEY, next);
          return next;
        });
        setLastUsage(usage);
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
    setSession(ZERO_TOTALS);
    setLastUsage(null);
    removeKey(SESSION_KEY);
  }, []);

  const handleClearAll = useCallback(() => {
    const confirmed =
      typeof window === "undefined"
        ? true
        : window.confirm(t("tokens.clearAllConfirm"));
    if (!confirmed) return;
    setSession(ZERO_TOTALS);
    setLifetime(ZERO_TOTALS);
    setLastUsage(null);
    removeKey(SESSION_KEY);
    removeKey(LIFETIME_KEY);
  }, [t]);

  // Finalize the current session on-disk, seed a fresh one, and re-hydrate
  // the Session panel from the new zeroed file. Lifetime stays as-is —
  // previous sessions still contribute to it.
  const handleNewSession = useCallback(() => {
    (async () => {
      try {
        await invoke<string>("new_session_cmd");
      } catch {
        // If the command fails, still clear the UI — the user's intent
        // was to start fresh. Backend will rotate on next restart.
      }
      try {
        const fresh = await invoke<SessionUsage>("get_current_session_usage");
        const next = sessionUsageToTotals(fresh);
        setSession(next);
        saveTotals(SESSION_KEY, next);
      } catch {
        setSession(ZERO_TOTALS);
        removeKey(SESSION_KEY);
      }
      setLastUsage(null);
    })();
  }, []);

  const hasSession = session.turns > 0;
  const hasLifetime = lifetime.turns > 0;
  const hasAny = hasSession || hasLifetime;

  // Tailwind utility groups (ADR-0016). `--accent-gemini` is a design token
  // that is not registered in the @theme bridge, so it (and the matching
  // translucent fill) are referenced via arbitrary values. The dt/dd rules
  // were descendant selectors of the (now-removed) cell class, so they are
  // applied directly to the elements here.
  const scopeLabel =
    "flex items-center gap-(--space-3) mt-0 mr-0 mb-(--space-2) ml-0 text-[9px] font-bold uppercase tracking-[0.6px] text-text-muted";
  const grid = "grid grid-cols-3 gap-x-[10px] gap-y-(--space-2) m-0";
  const cell = "flex flex-col min-w-0";
  const dt =
    "text-[9px] font-semibold uppercase tracking-[0.4px] text-text-muted m-0 leading-[1.2]";
  const dd =
    "font-['SF_Mono','Fira_Code','Consolas',monospace] text-sm font-semibold text-text-primary m-0 leading-[1.3] overflow-hidden text-ellipsis whitespace-nowrap";
  const ddTotal = `${dd} text-[var(--accent-gemini)] text-md`;
  const empty = "m-0 text-xs italic text-text-muted leading-[1.4]";

  return (
    <section
      className="flex-shrink-0 pt-(--space-4) px-(--space-5) pb-[10px] border-t border-border-color bg-bg-tertiary"
      aria-label={t("tokens.title")}
    >
      <div className="flex items-center justify-between mb-(--space-3) gap-(--space-4)">
        <h3 className="panel-title">{t("tokens.title")}</h3>
        <div className="flex items-center gap-(--space-3)">
          {hasSession && (
            <span
              className="text-2xs font-semibold bg-[rgb(52_211_153/0.15)] text-[var(--accent-gemini)] py-px px-(--space-4) rounded-[10px] tracking-[0.2px]"
              title={t("tokens.turnsTooltip")}
            >
              {t("tokens.turns", { count: session.turns })}
            </span>
          )}
          <button
            type="button"
            className="inline-flex items-center gap-(--space-2) py-[3px] px-(--space-4) text-2xs font-semibold tracking-[0.4px] uppercase text-text-secondary bg-[rgba(255,255,255,0.04)] border border-border-color rounded-md cursor-pointer transition-colors leading-[1.3] hover:not-disabled:text-accent-blue hover:not-disabled:bg-[rgba(96,165,250,0.1)] hover:not-disabled:border-[rgba(96,165,250,0.4)] disabled:opacity-40 disabled:cursor-not-allowed"
            onClick={handleNewSession}
            aria-label={t("tokens.newSession")}
            title={t("tokens.newSessionTooltip")}
          >
            {t("tokens.newSession")}
          </button>
          <button
            type="button"
            className="inline-flex items-center gap-(--space-2) py-[3px] px-(--space-4) text-2xs font-semibold tracking-[0.4px] uppercase text-text-secondary bg-[rgba(255,255,255,0.04)] border border-border-color rounded-md cursor-pointer transition-colors leading-[1.3] hover:not-disabled:text-accent-blue hover:not-disabled:bg-[rgba(96,165,250,0.1)] hover:not-disabled:border-[rgba(96,165,250,0.4)] disabled:opacity-40 disabled:cursor-not-allowed"
            onClick={handleReset}
            disabled={!hasSession}
            aria-label={t("tokens.reset")}
            title={t("tokens.reset")}
          >
            {t("tokens.reset")}
          </button>
          <button
            type="button"
            className="inline-flex items-center gap-(--space-2) py-[3px] px-(--space-4) text-2xs font-semibold tracking-[0.4px] uppercase text-text-secondary bg-[rgba(255,255,255,0.04)] border border-border-color rounded-md cursor-pointer transition-colors leading-[1.3] hover:not-disabled:text-accent-blue hover:not-disabled:bg-[rgba(96,165,250,0.1)] hover:not-disabled:border-[rgba(96,165,250,0.4)] disabled:opacity-40 disabled:cursor-not-allowed"
            onClick={handleClearAll}
            disabled={!hasAny}
            aria-label={t("tokens.clearAll")}
            title={t("tokens.clearAll")}
          >
            {t("tokens.clearAll")}
          </button>
        </div>
      </div>

      <fieldset
        className="mt-(--space-3) border-none p-0 m-0 min-w-0"
        aria-label={t("tokens.session")}
      >
        <h4 className={scopeLabel}>{t("tokens.session")}</h4>
        {!hasSession ? (
          <p className={empty}>{t("tokens.empty")}</p>
        ) : (
          <dl className={grid}>
            <div className={cell}>
              <dt className={dt}>{t("tokens.total")}</dt>
              <dd className={ddTotal}>{formatCount(session.total)}</dd>
            </div>
            <div className={cell}>
              <dt className={dt}>{t("tokens.prompt")}</dt>
              <dd className={dd}>{formatCount(session.prompt)}</dd>
            </div>
            <div className={cell}>
              <dt className={dt}>{t("tokens.response")}</dt>
              <dd className={dd}>{formatCount(session.response)}</dd>
            </div>
            {session.thoughts > 0 && (
              <div className={cell}>
                <dt className={dt}>{t("tokens.thoughts")}</dt>
                <dd className={dd}>{formatCount(session.thoughts)}</dd>
              </div>
            )}
            {session.toolUse > 0 && (
              <div className={cell}>
                <dt className={dt}>{t("tokens.toolUse")}</dt>
                <dd className={dd}>{formatCount(session.toolUse)}</dd>
              </div>
            )}
            {session.cached > 0 && (
              <div className={cell}>
                <dt className={dt}>{t("tokens.cached")}</dt>
                <dd className={dd}>{formatCount(session.cached)}</dd>
              </div>
            )}
          </dl>
        )}
      </fieldset>

      <fieldset
        className="mt-[10px] pt-(--space-4) border-0 border-t border-dashed border-border-color opacity-90 px-0 pb-0 m-0 min-w-0"
        aria-label={t("tokens.lifetime")}
      >
        <h4 className={scopeLabel}>
          {t("tokens.lifetime")}
          {hasLifetime && (
            <span
              className="text-[9px] font-semibold text-text-muted tracking-[0.2px] normal-case"
              title={t("tokens.turnsTooltip")}
            >
              {t("tokens.turns", { count: lifetime.turns })}
            </span>
          )}
        </h4>
        {!hasLifetime ? (
          <p className={empty}>{t("tokens.empty")}</p>
        ) : (
          <dl className={grid}>
            <div className={cell}>
              <dt className={dt}>{t("tokens.total")}</dt>
              <dd className={ddTotal}>{formatCount(lifetime.total)}</dd>
            </div>
            <div className={cell}>
              <dt className={dt}>{t("tokens.prompt")}</dt>
              <dd className={dd}>{formatCount(lifetime.prompt)}</dd>
            </div>
            <div className={cell}>
              <dt className={dt}>{t("tokens.response")}</dt>
              <dd className={dd}>{formatCount(lifetime.response)}</dd>
            </div>
            {lifetime.thoughts > 0 && (
              <div className={cell}>
                <dt className={dt}>{t("tokens.thoughts")}</dt>
                <dd className={dd}>{formatCount(lifetime.thoughts)}</dd>
              </div>
            )}
            {lifetime.toolUse > 0 && (
              <div className={cell}>
                <dt className={dt}>{t("tokens.toolUse")}</dt>
                <dd className={dd}>{formatCount(lifetime.toolUse)}</dd>
              </div>
            )}
            {lifetime.cached > 0 && (
              <div className={cell}>
                <dt className={dt}>{t("tokens.cached")}</dt>
                <dd className={dd}>{formatCount(lifetime.cached)}</dd>
              </div>
            )}
          </dl>
        )}
      </fieldset>

      {lastUsage && (
        <p
          className="mt-(--space-3) mb-0 text-2xs text-text-muted leading-[1.3]"
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
