/**
 * Notes panel — a structured, readable running summary of the conversation,
 * distinct from the raw transcription log.
 *
 * Two layers (ADR-0014):
 *   1. Synthesized notes (on demand) — the "Synthesize notes" button calls the
 *      backend `synthesize_notes` command, which reuses the chat LLM pipeline +
 *      the knowledge graph + transcript to produce a Markdown summary. Rendered
 *      above the base layer when present.
 *   2. Categorized base layer (always on) — derived purely on the client from
 *      existing store state (transcript segments + the typed knowledge graph),
 *      so it needs no backend call and updates live. It leans on the
 *      conversation ontology (ADR-0008): the graph's typed nodes
 *      (Question / Task / Decision / Topic / Person …) become readable chips.
 */
import { useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import {
  useReviewSessionId,
  useSessionView,
} from "../session/SessionViewProvider";
import { useActiveGraphSnapshot } from "../session/useActiveGraphSnapshot";
import { useAudioGraphStore } from "../store";
import type {
  GraphNode,
  GraphSnapshot,
  MaterializedNote,
  ProjectionPatch,
} from "../types";
import Button from "./Button";
import Icon from "./Icon";
import IconButton from "./IconButton";
import {
  type NotesSynthesisController,
  useNotesSynthesis,
} from "./useNotesSynthesis";

function byMention(a: GraphNode, b: GraphNode): number {
  return (b.mention_count ?? 0) - (a.mention_count ?? 0);
}

export interface NotesPanelProps {
  /**
   * Controlled synthesis state. Omit for the default, uncontrolled behavior
   * (NotesPanel owns its own `useNotesSynthesis()` instance and renders the
   * trigger in its header — the Capture/During and Analysis usages). Pass a
   * shared controller (and `headerActions={false}`) when the trigger lives
   * elsewhere — the Sessions detail's Notes lens.
   */
  synthesis?: NotesSynthesisController;
  /**
   * Whether NotesPanel renders its own header "Synthesize notes" trigger.
   * Defaults to `true`. The Sessions detail's Notes lens sets this `false`
   * and renders "Generate prose summary" in its own overflow instead (19c7
   * acceptance) — `data-notes-synthesize` and the header button leave
   * NotesPanel's header only in that one context.
   */
  headerActions?: boolean;
}

/** The graph a Review Notes pane derives chips from: none (see below). */
const NO_GRAPH_IN_REVIEW: GraphSnapshot = {
  nodes: [],
  links: [],
  stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
};

/** Stand-in controller for a pane that offers no synthesis (Review). */
const NO_SYNTHESIS: NotesSynthesisController = {
  loading: false,
  error: null,
  result: null,
  handleSynthesize: async () => {},
  clearError: () => {},
};

/**
 * The Notes lens. Synthesis (`synthesize_notes`) reads the backend's CURRENT
 * buffers, so a Review pane never gets a controller: it does not call
 * `useNotesSynthesis` at all, renders no trigger, result or error, and does
 * not offer the sample preview (`loadSampleSessionPreview` replaces the LIVE
 * session's content in the global store).
 */
export default function NotesPanel({
  synthesis,
  headerActions = true,
}: NotesPanelProps = {}) {
  const reviewing = useReviewSessionId() !== null;
  if (reviewing) {
    return <NotesPanelView synthesis={null} headerActions={false} reviewing />;
  }
  if (synthesis) {
    return (
      <NotesPanelView synthesis={synthesis} headerActions={headerActions} />
    );
  }
  return <OwnSynthesisNotesPanel headerActions={headerActions} />;
}

/** Uncontrolled: owns its own controller (Capture/During usage). */
function OwnSynthesisNotesPanel({ headerActions }: { headerActions: boolean }) {
  const synthesis = useNotesSynthesis();
  return <NotesPanelView synthesis={synthesis} headerActions={headerActions} />;
}

function NotesPanelView({
  synthesis,
  headerActions,
  reviewing = false,
}: {
  synthesis: NotesSynthesisController | null;
  headerActions: boolean;
  reviewing?: boolean;
}) {
  const { t, i18n } = useTranslation();
  const {
    transcriptSegments: segments,
    materializedNotes,
    sessionProjectionEvents: projectionEvents,
  } = useSessionView();
  // Merged snapshot, not raw `graphSnapshot` — see the comment in
  // `useNotesSynthesis`. Entity chips (Questions/Tasks/Decisions/
  // Topics) and the Person-node participant fallback below must read the
  // same graph the Graph lens shows, once it's loaded one.
  const { snapshot: activeGraph, materialized } = useActiveGraphSnapshot();
  // In Review the entity chips (and the Person-node participant fallback)
  // would come from the saved snapshot's live-lane graph, which is not
  // retraction-aware: it can still hold an item the materialized graph (what
  // the Graph lens shows) retracted. The Notes round does not read the
  // materialized graph, so a Review pane shows no graph-derived chips rather
  // than possibly-retracted ones; notes and transcript-derived participants
  // are unaffected.
  const graph =
    reviewing && materialized === null ? NO_GRAPH_IN_REVIEW : activeGraph;
  // Synthesis reads the backend's current buffers, so it is blocked while a
  // historical session is loaded into the global store. A Review pane shows
  // its own session, offers no synthesis, and holds no subscription to it.
  const loadedSessionId = useAudioGraphStore((s) =>
    reviewing ? null : s.loadedSessionId,
  );
  const loadSampleSessionPreview = useAudioGraphStore(
    (s) => s.loadSampleSessionPreview,
  );
  const { loading, error, result, handleSynthesize, clearError } =
    synthesis ?? NO_SYNTHESIS;
  const panelRef = useRef<HTMLDivElement>(null);

  // Stale when the graph or transcript has grown since the captured snapshot.
  const isStale =
    result !== null &&
    ((graph.nodes?.length ?? 0) !== result.nodeCount ||
      segments.length !== result.segmentCount);

  const synthesizedTime = useMemo(
    () => (result ? new Date(result.at).toLocaleTimeString() : ""),
    [result],
  );

  const dismissError = () => {
    clearError();
    // Focus-restoration to the trigger only applies when NotesPanel itself
    // renders one (see `headerActions` doc) — the Sessions-detail overflow
    // case has no `[data-notes-synthesize]` descendant to find.
    if (!headerActions) return;
    requestAnimationFrame(() => {
      panelRef.current
        ?.querySelector<HTMLButtonElement>("[data-notes-synthesize]")
        ?.focus();
    });
  };

  const notes = useMemo(() => {
    const nodes = graph.nodes ?? [];
    const ofType = (...types: string[]) =>
      nodes
        .filter((n) =>
          types.some((t) => n.entity_type?.toLowerCase() === t.toLowerCase()),
        )
        .sort(byMention);

    // Participants: prefer diarized speakers from the transcript; fall back to
    // Person nodes from the graph.
    const speakers = new Set<string>();
    for (const seg of segments) {
      if (seg.speaker_label) speakers.add(seg.speaker_label);
    }
    const participants =
      speakers.size > 0 ? [...speakers] : ofType("Person").map((n) => n.name);

    return {
      participants,
      questions: ofType("Question"),
      tasks: ofType("Task"),
      decisions: ofType("Decision"),
      topics: ofType("Topic", "Organization", "Product", "Event"),
    };
  }, [segments, graph]);

  // Issue #255 (ADR-0025 §2): a soft-invalidated note (`valid_until_ms`
  // set by an `invalidate_note` op) is hidden from the live panel but
  // stays auditable in materialized state — hidden-but-auditable, the same
  // posture as the graph lens's invalidated nodes.
  const liveNotes = (materializedNotes?.notes ?? []).filter(
    (note) => note.valid_until_ms == null,
  );
  const noteRevisionCounts = useMemo(
    () => notePatchRevisionCounts(projectionEvents),
    [projectionEvents],
  );

  const isEmpty =
    liveNotes.length === 0 &&
    notes.participants.length === 0 &&
    notes.questions.length === 0 &&
    notes.tasks.length === 0 &&
    notes.decisions.length === 0 &&
    notes.topics.length === 0;

  // Tailwind utility groups (ADR-0016). Colors/radii/fonts resolve through the
  // design tokens via the @theme bridge; spacing uses the token shorthand.
  const sectionTitle =
    "text-xs font-bold uppercase tracking-wide text-text-muted mb-[5px]";
  const chipBase =
    "text-sm py-[2px] px-(--space-4) rounded-xl bg-bg-elevated border border-(--edge)";

  return (
    <div ref={panelRef} className="flex h-full flex-col overflow-hidden">
      <div className="ag-panel-head shrink-0">
        <span className="ag-panel-head__title inline-flex items-center gap-(--space-2)">
          <Icon name="notes" size={16} /> {t("notes.title")}
        </span>
        {headerActions && synthesis !== null && (
          <Button
            variant="secondary"
            size="sm"
            icon="refresh"
            loading={loading}
            onClick={handleSynthesize}
            disabled={loadedSessionId !== null}
            aria-describedby={
              loadedSessionId ? "notes-review-synthesis-help" : undefined
            }
            data-notes-synthesize
            aria-label={
              result ? t("notes.refreshLabel") : t("notes.synthesizeLabel")
            }
          >
            {loading
              ? t("notes.synthesizing")
              : result
                ? t("notes.refresh")
                : t("notes.synthesize")}
          </Button>
        )}
      </div>

      <div className="flex flex-1 flex-col overflow-y-auto py-(--space-4) px-(--space-5)">
        {loadedSessionId && (
          <p
            id="notes-review-synthesis-help"
            className="m-0 mb-(--space-4) rounded-sm border border-(--tint-border-warning) bg-(--tint-warning) px-(--space-4) py-(--space-3) text-xs leading-[1.4] text-text-secondary"
            role="status"
          >
            <Icon name="warning" size={14} />{" "}
            {t("notes.reviewSynthesisBlocked")}
          </p>
        )}

        {error !== null && (
          <div
            role="alert"
            className="flex items-start gap-(--space-3) mb-(--space-4) py-(--space-3) px-(--space-4) rounded-lg bg-(--tint-danger) text-(--text-on-tint-danger) text-sm"
          >
            <Icon name="warning" size={16} />
            <span className="flex-1 [overflow-wrap:anywhere]">
              {t("notes.error", { message: error })}
            </span>
            <IconButton
              icon="close"
              label={t("notes.dismissError")}
              variant="ghost"
              className="bg-none border-none cursor-pointer shrink-0 opacity-70 hover:opacity-100"
              onClick={dismissError}
            />
          </div>
        )}

        {result !== null && (
          <section className="mb-(--space-5)">
            <div className="flex items-center justify-between gap-(--space-3) mb-[5px]">
              <h4 className={sectionTitle}>{t("notes.synthesized")}</h4>
              <span className="text-xs text-text-muted italic shrink-0">
                {t("notes.synthesizedAt", { time: synthesizedTime })}
              </span>
            </div>
            <div className="text-base text-text-primary whitespace-pre-wrap break-words py-(--space-4) px-(--space-5) rounded-lg bg-bg-tertiary border border-(--edge)">
              {result.markdown}
            </div>
            {isStale && (
              <p className="text-xs text-text-muted italic mt-(--space-2)">
                {t("notes.stale")}
              </p>
            )}
          </section>
        )}

        {isEmpty ? (
          <div
            className="flex flex-col items-center justify-center flex-1 gap-(--space-4) py-(--space-6) px-(--space-4) text-center select-none"
            data-testid="notes-empty-hero"
          >
            <span className="text-text-muted opacity-40" aria-hidden="true">
              <Icon name="notes" size={32} />
            </span>
            <div className="flex flex-col gap-(--space-2) max-w-[320px]">
              <p className="m-0 text-text-secondary text-md font-medium">
                {reviewing
                  ? t("sessions.review.emptyNotes")
                  : t("notes.emptyTitle")}
              </p>
              {!reviewing && (
                <p className="m-0 text-text-muted text-sm leading-normal">
                  {t("notes.empty")}
                </p>
              )}
            </div>
            {/* The sample preview replaces the LIVE session's content in the
                global store, so a reviewed (historical) session never offers it. */}
            {!reviewing && (
              <button
                type="button"
                className="inline-flex items-center gap-(--space-3) py-(--space-3) px-(--space-5) rounded-md text-sm font-semibold cursor-pointer bg-accent-blue text-(--on-accent-blue) border-none transition-opacity hover:opacity-90"
                onClick={() =>
                  loadSampleSessionPreview(
                    i18n.resolvedLanguage ?? i18n.language,
                  )
                }
              >
                <Icon name="start" size={16} />
                {t("notes.emptyPreviewSample")}
              </button>
            )}
          </div>
        ) : (
          <div className="flex flex-col gap-(--space-5)">
            {liveNotes.length > 0 && (
              <section>
                <div className="flex items-center justify-between gap-(--space-3) mb-[5px]">
                  <h4 className={sectionTitle}>{t("notes.materialized")}</h4>
                  <span className="text-xs text-text-muted italic shrink-0">
                    {t("notes.materializedSequence", {
                      sequence: materializedNotes?.last_sequence ?? 0,
                    })}
                  </span>
                </div>
                <ul className="list-none p-0 m-0 flex flex-col gap-(--space-3)">
                  {liveNotes.map((note) => (
                    <MaterializedNoteItem
                      key={note.id}
                      note={note}
                      revisionCount={noteRevisionCounts.get(note.id) ?? 0}
                    />
                  ))}
                </ul>
              </section>
            )}
            {notes.participants.length > 0 && (
              <section>
                <h4 className={sectionTitle}>{t("notes.participants")}</h4>
                <div className="flex flex-wrap gap-(--space-3)">
                  {notes.participants.map((p) => (
                    <span key={p} className={`${chipBase} text-text-primary`}>
                      {p}
                    </span>
                  ))}
                </div>
              </section>
            )}
            {notes.questions.length > 0 && (
              <NotesList
                title={t("notes.openQuestions")}
                items={notes.questions}
              />
            )}
            {notes.tasks.length > 0 && (
              <NotesList title={t("notes.actionItems")} items={notes.tasks} />
            )}
            {notes.decisions.length > 0 && (
              <NotesList title={t("notes.decisions")} items={notes.decisions} />
            )}
            {notes.topics.length > 0 && (
              <section>
                <h4 className={sectionTitle}>{t("notes.keyTopics")}</h4>
                <div className="flex flex-wrap gap-(--space-3)">
                  {notes.topics.slice(0, 12).map((n) => (
                    <span key={n.id} className={`${chipBase} text-accent-blue`}>
                      {n.name}
                      {n.mention_count > 1 ? ` ·${n.mention_count}` : ""}
                    </span>
                  ))}
                </div>
              </section>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function notePatchRevisionCounts(
  projectionEvents: ProjectionPatch[],
): Map<string, number> {
  const counts = new Map<string, number>();
  for (const patch of projectionEvents) {
    if (patch.kind !== "notes") continue;
    for (const operation of patch.operations) {
      if (
        operation.type !== "upsert_note" &&
        operation.type !== "delete_note" &&
        operation.type !== "reorder_note"
      ) {
        continue;
      }
      counts.set(operation.id, (counts.get(operation.id) ?? 0) + 1);
    }
  }
  return counts;
}

function MaterializedNoteItem({
  note,
  revisionCount,
}: {
  note: MaterializedNote;
  revisionCount: number;
}) {
  const { t } = useTranslation();
  const showRevision = revisionCount > 1;
  return (
    <li data-note-id={note.id} className="ag-card" data-elevation="flat">
      <div className="flex items-start justify-between gap-(--space-3)">
        <h5 className="m-0 text-sm font-semibold text-text-primary [overflow-wrap:anywhere]">
          {note.title}
        </h5>
        <span className="text-[11px] text-text-muted shrink-0">
          {t("notes.noteSequence", { sequence: note.updated_by_sequence })}
        </span>
      </div>
      {showRevision && (
        <p className="m-0 mt-[3px] text-[11px] leading-[1.35] text-accent-yellow">
          {t("notes.noteRevisions", { count: revisionCount })}
        </p>
      )}
      <p className="m-0 mt-(--space-2) text-base text-text-secondary whitespace-pre-wrap [overflow-wrap:anywhere]">
        {note.body}
      </p>
      {note.tags.length > 0 && (
        <div className="flex flex-wrap gap-(--space-2) mt-(--space-3)">
          {note.tags.map((tag) => (
            <span
              key={tag}
              className="text-[11px] py-[1px] px-(--space-2) rounded-sm bg-bg-elevated text-text-muted border border-(--edge)"
            >
              {tag}
            </span>
          ))}
        </div>
      )}
    </li>
  );
}

const SECTION_TITLE =
  "text-xs font-bold uppercase tracking-wide text-text-muted mb-[5px]";

function NotesList({ title, items }: { title: string; items: GraphNode[] }) {
  return (
    <section>
      <h4 className={SECTION_TITLE}>{title}</h4>
      <ul className="list-disc pl-(--space-6) flex flex-col gap-[3px]">
        {items.slice(0, 8).map((n) => (
          <li
            key={n.id}
            className="text-base text-text-primary [overflow-wrap:anywhere]"
          >
            {n.name}
          </li>
        ))}
      </ul>
    </section>
  );
}
