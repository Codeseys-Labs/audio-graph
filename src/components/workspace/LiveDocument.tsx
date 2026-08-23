/**
 * `LiveDocument` — the living-document renderer (ticket W5, synthesis
 * audio-graph-a6b5). Mounts INSIDE the bento document tile's
 * `WorkspaceTile`, replacing `NotesPanel` as that tile's body.
 * `NotesPanel` the FILE is untouched — it keeps hosting the Sessions
 * detail's Notes lens; only the live/reviewing document tile stops
 * rendering it (App.tsx).
 *
 * Two exports mount in two different places for the SAME reason design-a
 * §4.3/req 5 and this ticket's point 8 give: `WorkspaceTile`'s own header
 * bar already renders the tile's title ("Notes", `notes.title`, unchanged)
 * with a real accessible name — a second internal header repeating that
 * title would be the stacked-headers tension W4 already flagged, and (were
 * it a named landmark) a duplicate-region problem. So `LiveDocumentHeaderActions`
 * (note count + copy button) renders in `WorkspaceTile`'s `headerSlot`, and
 * `LiveDocument` renders ONLY the body — no internal header of its own, no
 * second named region. Both read the SAME `useLiveDocumentModel()` call
 * site's result via props from their shared caller (`App.tsx`) so there is
 * exactly one fold per render, not two.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionView } from "../../session/SessionViewProvider";
import { useAudioGraphStore } from "../../store";
import type { MaterializedNotes } from "../../types";
import Icon from "../Icon";
import IconButton from "../IconButton";
import Popover from "../Popover";
import {
  type DocNode,
  type DocSection,
  type LiveDocumentVM,
  notesToOutline,
  outlineToMarkdown,
} from "./liveDocumentModel";
import { laneRecencyChipTone, selectLaneRecency } from "./liveWorkspaceTone";

/**
 * Incrementally folds `materializedNotes` into a `LiveDocumentVM`, keyed on
 * referential identity so a fold only runs when the store actually hands
 * back a new snapshot (every accepted patch produces a new object —
 * `store/index.ts`'s `applyProjectionNotesPatch` — so this is precise, not
 * a heuristic). Mutating refs during render like this reads oddly, but is
 * SAFE under React 18 Strict Mode's double-invoke: the guard
 * (`foldedFor.current !== materializedNotes`) is false on a repeated
 * invocation with the same input (the first invocation already advanced
 * it), so a double-invoke just re-reads the already-folded value instead
 * of folding twice — no `useMemo`/`useEffect` double-fold hazard. A change
 * in `session_id` (switching live sessions, or loading a different
 * recorded one) resets the fold baseline so the newly-loaded document
 * doesn't render its entire content as "just changed."
 */
export function useLiveDocumentModel(): LiveDocumentVM {
  const { materializedNotes } = useSessionView();
  const sessionIdRef = useRef<string | null>(null);
  const foldedForRef = useRef<MaterializedNotes | null | undefined>(undefined);
  const vmRef = useRef<LiveDocumentVM | null>(null);

  const sessionId = materializedNotes?.session_id ?? null;
  if (sessionId !== sessionIdRef.current) {
    sessionIdRef.current = sessionId;
    vmRef.current = null;
    foldedForRef.current = undefined;
  }
  if (foldedForRef.current !== materializedNotes) {
    vmRef.current = notesToOutline(vmRef.current, materializedNotes);
    foldedForRef.current = materializedNotes;
  }
  return vmRef.current ?? notesToOutline(null, null);
}

function totalNoteCount(vm: LiveDocumentVM): number {
  let count = 0;
  for (const section of vm.sections) {
    for (const node of section.nodes) {
      if (node.depth === 0) count++;
    }
  }
  return count;
}

/** Header-slot content (note count + copy) — see the module doc for why
 * this renders in `WorkspaceTile`'s `headerSlot` rather than inside
 * `LiveDocument`'s own body. */
export function LiveDocumentHeaderActions({ vm }: { vm: LiveDocumentVM }) {
  const { t } = useTranslation();
  const copiedTimerRef = useRef<number | undefined>(undefined);
  const [copied, setCopied] = useState(false);

  const count = totalNoteCount(vm);

  const handleCopy = () => {
    const markdown = outlineToMarkdown(vm);
    navigator.clipboard?.writeText(markdown).then(
      () => {
        setCopied(true);
        window.clearTimeout(copiedTimerRef.current);
        copiedTimerRef.current = window.setTimeout(
          () => setCopied(false),
          1500,
        );
      },
      () => {
        // Clipboard write can fail (permissions/unsupported context); no
        // recovery path exists worth surfacing for a convenience action —
        // the user can still select and copy the rendered text manually.
      },
    );
  };

  return (
    <span className="flex items-center gap-(--space-3)">
      {count > 0 && (
        <span className="text-xs text-text-muted whitespace-nowrap">
          {t("document.noteCount", { count })}
        </span>
      )}
      <IconButton
        icon={copied ? "check" : "copy"}
        label={copied ? t("document.copied") : t("document.copy")}
        variant="ghost"
        size={14}
        onClick={handleCopy}
        disabled={count === 0}
      />
    </span>
  );
}

/**
 * The document tile's recency chip (ticket W6, synthesis audio-graph-a6b5
 * §2; evidence wired by ticket W3). Renders in `WorkspaceTile`'s
 * `headerSlot` ALONGSIDE `LiveDocumentHeaderActions` (composed by the
 * caller, `App.tsx`) — a SEPARATE component rather than folded into that
 * one because this reads different store state (`asrSpanRevisions`,
 * `loadedSessionId`, `sessionProjectionEvents`) that
 * `LiveDocumentHeaderActions` has no other use for.
 *
 * Honesty (L3/T2 law): `evidence` comes from `selectLaneRecency`'s own
 * mapping of the latest notes-lane patch's `basis_currency_at_apply` — so
 * `laneRecencyChipTone` returns `tone: "success"` ONLY when that patch
 * actually carried `{type: "current"}` (see `liveWorkspaceTone.ts`'s
 * module doc and its pinned tests). The visible chip text now has THREE
 * distinct renderings, one per honesty tier (design-a §2.4/§7,
 * `document.recency.current` = "Up to date"): the success "Up to date"
 * claim (`recency.tone === "success"`), the neutral "as of HH:MM:SS"
 * observed fact (no evidence yet, or evidence present but only proving an
 * append-only tail — `AppendedTail` stays neutral, never success), and the
 * warning "−N turns" escalation — never more than one at once, and never
 * the word "Live". Each rendering carries its own `.sr-only` text too, so
 * the success tier is never color-only (WCAG 1.4.1) — see this file's and
 * `liveWorkspaceTone.ts`'s fix-round comments for the gap this closed.
 */
export function DocRecencyChip() {
  const { t, i18n } = useTranslation();
  const { sessionProjectionEvents } = useSessionView();
  const asrSpanRevisions = useAudioGraphStore((s) => s.asrSpanRevisions);
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);

  // Memoized: `sessionProjectionEvents`/`asrSpanRevisions` only change when
  // the store actually appends a patch/revision, but this component's OWN
  // re-render cadence is driven by `useSessionView()`'s wider subscription
  // (includes `transcriptSegments`, several updates/sec live) — without this,
  // the O(patches+revisions) scan in `selectLaneRecency` would re-run on
  // every transcript tick, not just on the inputs that can change its result.
  const { lastAppliedAtMs, turnsBehind, evidence } = useMemo(
    () => selectLaneRecency("notes", sessionProjectionEvents, asrSpanRevisions),
    [sessionProjectionEvents, asrSpanRevisions],
  );
  const recency = laneRecencyChipTone({
    lastAppliedAtMs,
    turnsBehind,
    evidence,
    isLiveSession: loadedSessionId === null,
  });

  if (!recency.render || recency.lastAppliedAtMs === null) return null;

  // Explicit locale + `hour12: false`: an unqualified `toLocaleTimeString()`
  // resolves to the OS/runtime locale, not the app's i18n language — on an
  // en-US host that renders "11:59:59 PM" (AM/PM, 3 chars wider than the
  // budget test's assumed worst case) inside a pt sentence for a pt user.
  // Pinning both the locale and `hour12` keeps the rendered value
  // language-consistent AND fixed-width (`workspace-chip-length-budget.test.ts`).
  const time = new Date(recency.lastAppliedAtMs).toLocaleTimeString(
    i18n.language,
    { hour12: false },
  );
  // `recency.tone === "success"` is the ONLY path that can ever reach here
  // (`laneRecencyChipTone`'s `automaticProbeAvailable: evidence === "current"`
  // gate) — `recency.behind` is checked first because the ≥3-turns warning
  // always wins regardless of evidence (design-a §2.4's table).
  const label = recency.behind
    ? t("document.recency.behind", { count: recency.turnsBehind })
    : recency.tone === "success"
      ? t("document.recency.current")
      : t("document.recency.asOf", { time });
  const ariaLabel = recency.behind
    ? t("document.recency.behindAria", { count: recency.turnsBehind })
    : recency.tone === "success"
      ? t("document.recency.currentAria", { time })
      : t("document.recency.asOfAria", { time });

  return (
    // `aria-label` on a bare `<span>` (implicit "generic" role) is invalid
    // ARIA — the accessible-name computation excludes generic roles, and
    // biome's `useAriaPropsSupportedByRole` lint catches it. Mirrors
    // `PipelineStatusBar.tsx`'s established idiom for a text-bearing chip
    // (A11Y-1): the visible label stays `aria-hidden`, and a `.sr-only`
    // sibling carries the fuller explanation — no `role="img"` (that's
    // reserved for genuinely empty/color-only elements, per that file's own
    // comment), no `aria-live` (W10 owns announcements).
    <span className="ag-chip" data-tone={recency.tone}>
      <span aria-hidden="true">{label}</span>
      <span className="sr-only">{ariaLabel}</span>
    </span>
  );
}

function DocNodeGutter({ node }: { node: DocNode }) {
  const { t } = useTranslation();
  const showGlyph = node.revisions > 1;
  return (
    <Popover
      side="left"
      align="start"
      trigger={
        <button
          type="button"
          className="ag-btn-micro opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 shrink-0"
          aria-label={t("document.gutterLabel")}
        >
          {showGlyph ? `·${node.revisions}` : ""}
        </button>
      }
    >
      <div className="flex flex-col gap-(--space-2) min-w-[160px] text-xs">
        {node.revisions > 1 && (
          <p className="m-0 text-text-secondary">
            {t("notes.noteRevisions", { count: node.revisions })}
          </p>
        )}
        <p className="m-0 text-text-muted">
          {t("notes.noteSequence", { sequence: node.seq })}
        </p>
        {node.tags.length > 0 && (
          <div className="flex flex-wrap gap-(--space-1)">
            {node.tags.map((tag) => (
              <span key={tag} className="ag-chip" data-tone="neutral">
                {tag}
              </span>
            ))}
          </div>
        )}
      </div>
    </Popover>
  );
}

function DocBullet({ node }: { node: DocNode }) {
  return (
    <li
      data-note-id={node.id}
      className={
        node.depth === 0
          ? "list-none"
          : node.depth === 1
            ? "list-none ml-(--space-6)"
            : "list-none ml-(--space-8)"
      }
    >
      <div className="group flex items-start justify-between gap-(--space-3) py-[2px]">
        <p className="m-0 text-base text-text-primary [overflow-wrap:anywhere]">
          {node.lead && <strong>{node.lead}</strong>}
          {node.lead ? " " : ""}
          {node.text}
        </p>
        <DocNodeGutter node={node} />
      </div>
    </li>
  );
}

function DocSectionView({ section }: { section: DocSection }) {
  const HeadingTag =
    section.headingLevel != null
      ? (`h${section.headingLevel}` as "h2" | "h3" | "h4")
      : null;
  return (
    <section className="mb-(--space-5)" data-section-id={section.id}>
      {HeadingTag && section.heading !== null && (
        <HeadingTag className="m-0 mb-(--space-2) text-sm font-semibold text-text-primary">
          {section.heading}
        </HeadingTag>
      )}
      {section.nodes.length > 0 && (
        <ul className="list-none p-0 m-0 flex flex-col gap-(--space-1)">
          {section.nodes.map((node) => (
            <DocBullet key={node.id} node={node} />
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * The document tile's body — a typed outline, keyed by stable node id so an
 * unchanged node never remounts. L1 (no autoscroll on rewrite): follows the
 * bottom ONLY when the newest fold is a pure tail append (`appendedAtTail`)
 * AND the reader is within 100px of the bottom — the exact idiom
 * `LiveTranscript.tsx` uses for the transcript tile: `wasNearBottomRef`
 * starts `true` (a fresh mount with nothing scrolled yet is, by definition,
 * "at the bottom") and a real scroll away from the tail is the only thing
 * that ever flips it false. A mid-document rewrite never moves the
 * viewport, by construction (a rewrite anywhere makes `appendedAtTail`
 * false).
 */
export function LiveDocument({ vm }: { vm: LiveDocumentVM }) {
  const { t, i18n } = useTranslation();
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const loadSampleSessionPreview = useAudioGraphStore(
    (s) => s.loadSampleSessionPreview,
  );
  const scrollRef = useRef<HTMLDivElement>(null);
  const wasNearBottomRef = useRef(true);

  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    wasNearBottomRef.current =
      el.scrollHeight - el.scrollTop - el.clientHeight < 100;
  };

  // biome-ignore lint/correctness/useExhaustiveDependencies: vm.lastSequence is the intentional re-run trigger (a new fold), mirroring LiveTranscript.tsx's own auto-follow effect
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (vm.appendedAtTail && wasNearBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [vm.lastSequence]);

  if (vm.sections.length === 0) {
    // design-a §1.7 row 2: while actively capturing and nothing has landed
    // yet, show three aria-hidden skeleton rows plus an sr-only "building"
    // announcement — NOT a spinner, and not the "nothing started yet" hero
    // below (which would otherwise falsely suggest capture hasn't begun).
    if (isCapturing) {
      return (
        <div
          className="flex flex-col gap-(--space-3) py-(--space-4) px-(--space-5)"
          data-testid="document-skeleton"
        >
          <span className="sr-only" role="status">
            {t("document.buildingNotes")}
          </span>
          <div aria-hidden="true" className="flex flex-col gap-(--space-3)">
            <div className="h-[14px] w-[85%] rounded-sm bg-(--hover-overlay) animate-pulse" />
            <div className="h-[14px] w-[70%] rounded-sm bg-(--hover-overlay) animate-pulse" />
            <div className="h-[14px] w-[60%] rounded-sm bg-(--hover-overlay) animate-pulse" />
          </div>
        </div>
      );
    }
    return (
      <div
        className="flex flex-col items-center justify-center h-full gap-(--space-4) py-(--space-6) px-(--space-4) text-center select-none"
        data-testid="document-empty"
      >
        <span className="text-text-muted opacity-40" aria-hidden="true">
          <Icon name="notes" size={32} />
        </span>
        <div className="flex flex-col gap-(--space-2) max-w-[320px]">
          <p className="m-0 text-text-secondary text-md font-medium">
            {t("notes.emptyTitle")}
          </p>
          <p className="m-0 text-text-muted text-sm leading-normal">
            {t("document.empty")}
          </p>
        </div>
        <button
          type="button"
          className="inline-flex items-center gap-(--space-3) py-(--space-3) px-(--space-5) rounded-md text-sm font-semibold cursor-pointer bg-accent-blue text-(--on-accent-blue) border-none transition-opacity hover:opacity-90"
          onClick={() =>
            loadSampleSessionPreview(i18n.resolvedLanguage ?? i18n.language)
          }
        >
          <Icon name="start" size={16} />
          {t("notes.emptyPreviewSample")}
        </button>
      </div>
    );
  }

  return (
    <div
      ref={scrollRef}
      onScroll={handleScroll}
      className="h-full overflow-y-auto py-(--space-4) px-(--space-5)"
    >
      {vm.sections.map((section) => (
        <DocSectionView key={section.id} section={section} />
      ))}
    </div>
  );
}

export default LiveDocument;
