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
  type AnchorSplit,
  computeScrollTopToCenter,
  type NodeGeometry,
  nearestNodeId,
  splitByDirection,
  updateUnseenChangedIds,
  type ViewportGeometry,
} from "./docChangeAnchor";
import {
  type BatchedChangeAnnouncer,
  createBatchedChangeAnnouncer,
} from "./docChangeAnnouncer";
import {
  type DocNode,
  type DocSection,
  type LiveDocumentVM,
  notesToOutline,
  outlineToMarkdown,
} from "./liveDocumentModel";
import { laneRecencyChipTone, selectLaneRecency } from "./liveWorkspaceTone";

/** Ticket W10 (synthesis audio-graph-a6b5 §2's L2 law). This value is
 * RATIFIED, not a disclosed placeholder: design-a §1.6 says the region is
 * "debounced 2000ms so a burst of ticks collapses to one utterance" and the
 * synthesis (§W10) repeats "debounced 2s sr-only 'N passages refined'" —
 * both name 2000ms explicitly. Documented here (rather than only in
 * `docChangeAnnouncer.ts`) as the one place a future ticket would retune
 * it, should design-a itself ever change the constant. */
const DOC_ANNOUNCE_DEBOUNCE_MS = 2000;

/** design-a §1.4's FIRST rate limit: "if more than 6 nodes change in one
 * tick, no pulses at all — just the header count." A document strobing six
 * places at once communicates less than a number would. Does NOT gate the
 * change anchor or the sr-only announcement — both still report the real
 * count of changed nodes internally (`vm.changedNodeIds.length`, uncapped);
 * only the per-node visual pulse is suppressed above this threshold. NOTE:
 * the document tile's HEADER (`LiveDocumentHeaderActions`, `document.noteCount`)
 * renders the total note count, not the changed-node count — design-a's
 * "just the header count" fallback for a >6-node fold has no dedicated
 * sighted-user surface today; see that gap's tracking note rather than
 * reading this comment as a claim that one exists.
 *
 * design-a §1.4 also names a SECOND rate limit this file does not
 * implement: "at most one pulse per node per 1.5s window." The class-identity
 * retrigger guard below (`DocBullet`'s own doc) already prevents an
 * unrelated re-render from restarting a node's animation, and the
 * `changedAtSeq`-keyed remount two paragraphs down guarantees a genuine
 * back-to-back content change DOES restart it — but nothing here throttles
 * two genuine changes to the SAME node inside a sub-1.5s window; both would
 * pulse. Left as a disclosed gap rather than an undisclosed one. */
const MAX_PULSING_NODES = 6;

const EMPTY_PULSE_IDS: ReadonlySet<string> = new Set();

/** Reads the ACTUAL DOM geometry for one previously-rendered note row, in
 * the scroll container's own coordinate space — the one non-pure step
 * `docChangeAnchor.ts`'s module doc names as the sole reason this file, not
 * that one, needs a DOM. Returns `null` when the id isn't currently
 * rendered (deleted, or a stale id left over from a different session's
 * fold — `updateUnseenChangedIds`/`splitByDirection` both already treat a
 * `null` entry as "drop it", so this is the single place that garbage
 * collection is anchored). */
function measureNodeGeometry(
  container: HTMLElement,
  id: string,
): NodeGeometry | null {
  const el = container.querySelector<HTMLElement>(`[data-note-id="${id}"]`);
  if (!el) return null;
  const containerRect = container.getBoundingClientRect();
  const elRect = el.getBoundingClientRect();
  return {
    offsetTop: elRect.top - containerRect.top + container.scrollTop,
    offsetHeight: elRect.height,
  };
}

function readViewportGeometry(container: HTMLElement): ViewportGeometry {
  return {
    scrollTop: container.scrollTop,
    clientHeight: container.clientHeight,
  };
}

function measureMany(
  container: HTMLElement,
  ids: Iterable<string>,
): Map<string, NodeGeometry | null> {
  const geometryById = new Map<string, NodeGeometry | null>();
  for (const id of ids)
    geometryById.set(id, measureNodeGeometry(container, id));
  return geometryById;
}

/**
 * Shared by both trigger sites (a new fold, and a scroll event) so they
 * can't drift into two different notions of "out of view" — `newlyChangedIds`
 * is `[]` for the scroll-triggered call (nothing NEW changed; a scroll can
 * only ever REMOVE tracked ids by carrying them into view, never add one).
 */
function recomputeUnseenChanges(
  container: HTMLElement,
  previouslyUnseen: ReadonlySet<string>,
  newlyChangedIds: readonly string[],
): { next: Set<string>; split: AnchorSplit } {
  const viewport = readViewportGeometry(container);
  const geometryById = measureMany(container, [
    ...previouslyUnseen,
    ...newlyChangedIds,
  ]);
  const next = updateUnseenChangedIds(
    previouslyUnseen,
    newlyChangedIds,
    viewport,
    geometryById,
  );
  return { next, split: splitByDirection(next, viewport, geometryById) };
}

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

/**
 * `pulsing` is derived ONLY from `vm.changedNodeIds.includes(node.id)`
 * (via the caller's `pulsingIds` set, capped by `MAX_PULSING_NODES`) — a
 * value that is, by `liveDocumentModel.ts`'s own contract, scoped to
 * "hash-changed in the newest patch only" (never accumulated). That's what
 * makes this safe from the ticket's "must not retrigger on unrelated
 * re-renders" requirement WITHOUT any local timer/state in this component:
 * an unrelated re-render (the store's `vm` reference unchanged) recomputes
 * the exact same boolean, so React never touches the `className` DOM
 * attribute, and a CSS `animation` only ever (re)starts when a class
 * attribute's VALUE actually changes — never merely because a render
 * happened.
 *
 * That same class-identity guard has a real gap, though: a node that
 * pulses in fold N and pulses AGAIN in fold N+1 (no intervening fold that
 * excludes it from `changedNodeIds`) renders the identical className
 * string — `"...ag-doc-refined"` — in BOTH folds, so React never touches
 * the class attribute the second time either, and the one-shot animation
 * never restarts (confirmed: verified with a real gap of tens of seconds
 * between the two folds — zero attribute mutations, no second pulse).
 * `key={pulseKey}` below closes that gap: keyed on `node.changedAtSeq`
 * (bumped by `liveDocumentModel.ts`'s `foldNode` every time — and ONLY
 * when — this node's own `contentHash` changes) while `pulsing`, a
 * consecutive-fold re-refinement gets a NEW key value, so React unmounts
 * the old wrapper `div` and mounts a fresh one with the class already
 * present at creation — a brand-new element always plays its animation
 * from the start, no attribute-value change needed to notice. Falls back
 * to a stable `"static"` key while not pulsing so the common case (nothing
 * changed) never remounts anything, preserving the "no remount of an
 * unchanged node" contract this file's own tests pin elsewhere. The
 * animation's own 1.5s one-shot duration (`layout.css`) naturally stops
 * rendering any visible effect long before the NEXT fold arrives in every
 * realistic tick cadence, so no separate removal timer is needed.
 */
function DocBullet({ node, pulsing }: { node: DocNode; pulsing: boolean }) {
  const pulseKey = pulsing ? `pulse-${node.changedAtSeq}` : "static";
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
      <div
        key={pulseKey}
        className={`group flex items-start justify-between gap-(--space-3) py-[2px]${
          pulsing ? " ag-doc-refined" : ""
        }`}
      >
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

function DocSectionView({
  section,
  pulsingIds,
}: {
  section: DocSection;
  pulsingIds: ReadonlySet<string>;
}) {
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
            <DocBullet
              key={node.id}
              node={node}
              pulsing={pulsingIds.has(node.id)}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * The out-of-viewport change anchor (design-a §1.6, ticket W10). ALWAYS
 * mounted (never conditionally unmounted by the caller) — a `dissolve` is
 * an opacity TRANSITION on a still-present element (`layout.css`'s
 * `.ag-doc-anchor` rule), and a conditionally-unmounted element has nothing
 * to transition FROM. `count === 0` renders it `aria-hidden`,
 * `pointer-events: none`, and `opacity: 0`, keeping the LAST non-zero
 * `count` as its label text while fading (`displayCount` below) rather than
 * flashing to "0 updated above" for the duration of the fade.
 *
 * L1/L2 discipline: this is a real `<button>` (keyboard-reachable,
 * `useButtonType`-compliant). Its own click handler (passed in by the
 * caller) scrolls the CONTAINER only and never calls `.focus()` on the
 * target note — the document's content is never a focus target. It also
 * never leaves focus stranded here, though: this same click dissolves the
 * button (`aria-hidden="true"`, `tabIndex={-1}`) once the jump lands, and
 * an aria-hidden element that still holds DOM focus is axe's
 * `aria-hidden-focus` violation (WCAG 4.1.2) — a screen reader user's focus
 * would sit on a node the accessibility tree no longer exposes. `jumpTo`
 * (below) moves focus to the scroll container itself instead — a neutral,
 * non-note target — before this button's own `aria-hidden` flips. Nothing
 * here ever fires on mount/fold by itself; it only reacts to an explicit
 * click.
 */
function DocChangeAnchor({
  direction,
  count,
  onClick,
}: {
  direction: "above" | "below";
  count: number;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  const [displayCount, setDisplayCount] = useState(count);
  useEffect(() => {
    if (count > 0) setDisplayCount(count);
  }, [count]);
  const visible = count > 0;
  const label =
    direction === "above"
      ? t("document.changeAnchor.above", { count: displayCount })
      : t("document.changeAnchor.below", { count: displayCount });
  return (
    <button
      type="button"
      className="ag-doc-anchor"
      data-direction={direction}
      aria-hidden={!visible}
      tabIndex={visible ? 0 : -1}
      style={{
        opacity: visible ? 1 : 0,
        pointerEvents: visible ? "auto" : "none",
      }}
      onClick={onClick}
    >
      {label}
    </button>
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

  // Ticket W10: the ids THIS fold changed, capped per design-a §1.4's
  // strobe-suppression rule (`MAX_PULSING_NODES`) — a `Set` so `DocBullet`'s
  // per-node membership check is O(1). `useMemo` keyed on `vm.changedNodeIds`
  // ITSELF (not `vm`) means an unrelated re-render with the same `vm`
  // (same array reference) returns the SAME `Set` instance, which is what
  // keeps `DocBullet`'s rendered `className` string byte-identical across
  // those re-renders — see that component's own doc for why that's what
  // stops the pulse from retriggering.
  const pulsingIds = useMemo(
    () =>
      vm.changedNodeIds.length > MAX_PULSING_NODES
        ? EMPTY_PULSE_IDS
        : new Set(vm.changedNodeIds),
    [vm.changedNodeIds],
  );

  // Ticket W10 (design-a §1.6's `DocChangeAnchor` + the debounced sr-only
  // announcement). `unseenIdsRef` is the persisted-but-not-itself-rendering
  // tracked set (see `docChangeAnchor.ts`'s module doc); `anchorSplit` is
  // the derived, RENDERED snapshot, recomputed whenever the tracked set
  // could plausibly have changed (a new fold, or a scroll that may have
  // carried a tracked node into or out of view).
  const unseenIdsRef = useRef<Set<string>>(new Set());
  const [anchorSplit, setAnchorSplit] = useState<AnchorSplit>({
    above: [],
    below: [],
  });
  const [announcedText, setAnnouncedText] = useState("");
  const announcerRef = useRef<BatchedChangeAnnouncer | null>(null);
  // The announcer's flush callback can fire on a timer well after any
  // particular render — a ref keeps it reading the LATEST `t` (e.g. after a
  // live language change) without recreating the announcer, which would
  // drop its pending window, every time `t`'s own identity changes.
  const tRef = useRef(t);
  tRef.current = t;
  // Tracks the last flush's rendered BASE text (never including the marker
  // below) plus whether that flush appended one — lets a LATER flush with
  // the exact same passage count still mutate the live region.
  const lastAnnouncedRef = useRef<{ text: string; marked: boolean }>({
    text: "",
    marked: false,
  });
  if (announcerRef.current === null) {
    announcerRef.current = createBatchedChangeAnnouncer((count) => {
      const base = tRef.current("document.a11y.changed", { count });
      const prior = lastAnnouncedRef.current;
      // A steady-state live session (one passage refined per debounce
      // window, repeatedly) can flush the SAME count window after window —
      // "1 passage refined" then, two seconds later, "1 passage refined"
      // again. `setAnnouncedText` with a byte-identical string is a no-op
      // (`Object.is` bails, same as any other `useState` setter), so React
      // never touches the live region's DOM text, and a MutationObserver
      // (which is what actually drives a screen reader's aria-live
      // announcement) sees nothing — the reader hears the FIRST window and
      // then silence forever after, even though real refinements kept
      // happening. Appending a trailing zero-width space (invisible, and
      // not vocalized by screen readers) on every OTHER repeat of the same
      // base text guarantees the rendered string differs from what's
      // already in the DOM, so the mutation — and the announcement — always
      // happens; alternating (rather than always appending) keeps a
      // genuinely NEW count's rendering byte-identical to before this fix,
      // so it never gains a marker it doesn't need.
      const marked = base === prior.text ? !prior.marked : false;
      lastAnnouncedRef.current = { text: base, marked };
      setAnnouncedText(marked ? `${base}\u200b` : base);
    }, DOC_ANNOUNCE_DEBOUNCE_MS);
  }
  useEffect(() => {
    const announcer = announcerRef.current;
    return () => announcer?.cancel();
  }, []);

  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    wasNearBottomRef.current =
      el.scrollHeight - el.scrollTop - el.clientHeight < 100;
    // Cheap guard: with nothing tracked and nothing currently shown, a
    // scroll event cannot possibly change anything the anchor cares about.
    // Without this, `splitByDirection` always returns a FRESH `{above: [],
    // below: []}` object even when both arrays stay empty, so `Object.is`
    // never bails and `setAnchorSplit` re-renders the WHOLE outline
    // (`DocSectionView`/`DocBullet` are unmemoized) on every single scroll
    // tick, even in the steady-state case where nothing has changed at all.
    // This also skips the O(tracked ids) `querySelector` +
    // `getBoundingClientRect` reads below for that same common case.
    if (
      unseenIdsRef.current.size === 0 &&
      anchorSplit.above.length === 0 &&
      anchorSplit.below.length === 0
    ) {
      return;
    }
    // A scroll can only ever REMOVE tracked ids (by carrying them into
    // view) — never add one — so this passes no `newlyChangedIds`.
    const { next, split } = recomputeUnseenChanges(
      el,
      unseenIdsRef.current,
      [],
    );
    unseenIdsRef.current = next;
    setAnchorSplit(split);
  };

  // biome-ignore lint/correctness/useExhaustiveDependencies: vm.lastSequence is the intentional re-run trigger (a new fold), mirroring LiveTranscript.tsx's own auto-follow effect
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (vm.appendedAtTail && wasNearBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [vm.lastSequence]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: vm.lastSequence is the intentional re-run trigger (a new fold) — mirrors the tail-follow effect above; vm.changedNodeIds/appendedAtTail always change together with it (one fold, one VM)
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const { next, split } = recomputeUnseenChanges(
      el,
      unseenIdsRef.current,
      vm.changedNodeIds,
    );
    unseenIdsRef.current = next;
    setAnchorSplit(split);

    // Ticket W10's disclosed choice: a pure tail append is already
    // surfaced visually by the sticky-follow effect above — announcing it
    // too would interrupt the reader for exactly the case that needs it
    // least (new content arriving where they're already looking).
    if (!vm.appendedAtTail) {
      announcerRef.current?.push(vm.changedNodeIds);
    }
  }, [vm.lastSequence]);

  const jumpTo = (direction: "above" | "below") => {
    const el = scrollRef.current;
    if (!el) return;
    const ids = direction === "above" ? anchorSplit.above : anchorSplit.below;
    const geometryById = measureMany(el, ids);
    const targetId = nearestNodeId(ids, direction, geometryById);
    const geometry = targetId !== null ? geometryById.get(targetId) : null;
    if (!geometry) return;
    // Sets `scrollTop` directly rather than the target note's own
    // `scrollIntoView({ behavior: scrollBehavior(), block: "center" })`
    // (design-a §1.6's named mechanism, and the idiom `LiveTranscript.tsx`'s
    // own click-to-jump already uses) — a disclosed deviation, not a
    // focus-safety requirement: `Element.scrollIntoView` moves the viewport
    // only and would NOT itself steal focus. The reason for the manual
    // `computeScrollTopToCenter` math instead is that it's the same
    // DOM-free, unit-testable geometry `docChangeAnchor.ts` already owns
    // for `nearestNodeId`, so this call site never needs real
    // `getBoundingClientRect` centering logic of its own; a future ticket
    // could switch to `scrollIntoView` + `motion.ts`'s `scrollBehavior()`
    // without changing this file's L1/L2 guarantees.
    el.scrollTop = computeScrollTopToCenter(
      readViewportGeometry(el),
      geometry,
      el.scrollHeight,
    );
    const { next, split } = recomputeUnseenChanges(
      el,
      unseenIdsRef.current,
      [],
    );
    unseenIdsRef.current = next;
    setAnchorSplit(split);
    // Move focus to the scroll container itself — a neutral, non-note
    // target (`tabIndex={-1}` below makes it programmatically focusable
    // without adding a new stop to the Tab order, the same idiom
    // `SystemDrawer.tsx`'s dialog root uses) — rather than letting it
    // strand on the anchor button once that button's own `aria-hidden`
    // flips to `"true"` a moment later. `preventScroll: true` because the
    // scroll position was JUST set above; a default-behavior `.focus()`
    // call re-scrolling the container into its own view would fight that.
    el.focus({ preventScroll: true });
  };

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
      // Programmatic-only focus target for `jumpTo`'s post-scroll focus
      // move (see that function's own comment) — `-1` keeps it out of the
      // Tab order; nothing here relies on it ever receiving REAL keyboard
      // focus.
      tabIndex={-1}
      className="relative h-full overflow-y-auto py-(--space-4) px-(--space-5)"
    >
      {/* Ticket W10: EXACTLY ONE aria-live=polite region for the whole
          document tile, mounted for the entire lifetime of THIS branch (the
          non-empty outline) rather than remounted per fold — a screen
          reader keeps one stable region to watch across every fold that
          fires while there IS an outline, rather than re-discovering a
          fresh one every time. NARROWER than "for the whole tile": the
          empty/skeleton early-return branches above render a DIFFERENT
          `role="status"` node (no `aria-live`) and mount/unmount this one
          across the empty<->content boundary — `vm.changedNodeIds` is
          always empty on the fold that crosses that boundary either
          direction (`liveDocumentModel.ts`'s `hadPriorRenderedContent`
          guard), so no real announcement is lost by that transition, but a
          screen reader technically re-discovers the region at that moment.
          Empty text before the first flush is silent (nothing to announce
          yet, and an empty aria-live region announces nothing). */}
      <span className="sr-only" role="status" aria-live="polite">
        {announcedText}
      </span>
      {/* Always mounted (see `DocChangeAnchor`'s own doc) — visibility is an
          opacity/pointer-events toggle, not a mount/unmount, so the CSS
          dissolve transition has something to animate FROM. */}
      <DocChangeAnchor
        direction="above"
        count={anchorSplit.above.length}
        onClick={() => jumpTo("above")}
      />
      {vm.sections.map((section) => (
        <DocSectionView
          key={section.id}
          section={section}
          pulsingIds={pulsingIds}
        />
      ))}
      <DocChangeAnchor
        direction="below"
        count={anchorSplit.below.length}
        onClick={() => jumpTo("below")}
      />
    </div>
  );
}

export default LiveDocument;
