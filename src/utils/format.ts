/**
 * Shared formatting utilities for the AudioGraph frontend.
 *
 * Consolidates duplicated `formatTime` helpers that previously lived in
 * SpeakerPanel, KnowledgeGraphViewer, and LiveTranscript, and (seed e7e5,
 * SHELL-R2) the duplicated hours-aware duration formatter — see
 * `formatDurationHoursAware` below.
 */

/** Format seconds as `M:SS` (e.g. `"2:05"`). */
export function formatTime(seconds: number): string {
  if (!seconds && seconds !== 0) return "—";
  const mins = Math.floor(seconds / 60);
  const secs = Math.floor(seconds % 60);
  return mins > 0
    ? `${mins}:${secs.toString().padStart(2, "0")}`
    : `0:${secs.toString().padStart(2, "0")}`;
}

/**
 * Format a duration in seconds as an hours-aware human string: `"3h 12m"`,
 * `"5m 30s"`, or `"42s"` — the one shared h/m/s convention SHELL-R2 (seed
 * audio-graph-e0c4) folds from ALL THREE diverging copies seed e7e5 named:
 * SessionsBrowser's own `formatDuration(seconds)` (session length),
 * ProjectionRuntimeStatusPanel's `formatAgeMs(ms)` (scheduler lag), and
 * SpeakerPanel's `formatDuration(seconds)` (per-speaker talk time). All
 * three now delegate here instead of re-deriving the same three properties;
 * the module-level `formatDuration` helper they used to share is retired
 * (no remaining callers), closing e7e5's "60m 0s" vs "1h 0m" inconsistency
 * for good rather than leaving one copy live.
 *
 * `null`/`undefined` (no measurement yet) renders as `"—"`; a non-finite or
 * non-positive value clamps to `"0s"` rather than propagating `NaN` into the
 * UI — the three call sites disagreed on this edge (SessionsBrowser and
 * SpeakerPanel didn't guard it at all; ProjectionRuntimeStatusPanel guarded
 * non-finite/≤0), so this is the strictest of the three, applied uniformly.
 */
export function formatDurationHoursAware(
  seconds: number | null | undefined,
): string {
  if (seconds === null || seconds === undefined) return "—";
  if (!Number.isFinite(seconds) || seconds <= 0) return "0s";
  const total = Math.floor(seconds);
  if (total < 60) return `${total}s`;
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m ${secs}s`;
}

/**
 * Format a past unix-millis timestamp as a short relative-time string
 * (`"2 hours ago"`, `"yesterday"`, `"just now"`) via `Intl.RelativeTimeFormat`
 * — the Sessions rail's row timestamp (SHELL-R2, plan §R2, ADR-0046), which reads
 * as a noun's age rather than an absolute clock time.
 *
 * `now` defaults to `Date.now()` but is an explicit param so callers/tests
 * can pin it instead of racing the real clock. A falsy `ms` (no timestamp
 * yet) renders as `"—"`, matching this module's existing convention.
 */
export function formatRelativeTime(
  ms: number,
  now: number = Date.now(),
  locale?: string,
): string {
  if (!ms) return "—";
  const diffSeconds = Math.round((ms - now) / 1000);
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  for (const [unit, secondsInUnit] of RELATIVE_TIME_UNITS) {
    if (Math.abs(diffSeconds) >= secondsInUnit || unit === "second") {
      return rtf.format(Math.round(diffSeconds / secondsInUnit), unit);
    }
  }
  // Unreachable — the loop always terminates on "second" — but keeps the
  // function total for TypeScript's control-flow analysis.
  return rtf.format(0, "second");
}

const RELATIVE_TIME_UNITS: ReadonlyArray<
  [Intl.RelativeTimeFormatUnit, number]
> = [
  ["year", 365 * 24 * 60 * 60],
  ["month", 30 * 24 * 60 * 60],
  ["week", 7 * 24 * 60 * 60],
  ["day", 24 * 60 * 60],
  ["hour", 60 * 60],
  ["minute", 60],
  ["second", 1],
];
