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
// `safeInvoke` (aliased to `invoke`) is a drop-in for the Tauri `invoke` that
// relays a command-name-only failure diagnostic to analytics then rethrows, so
// this call site's error handling is unchanged (audio-graph-3e71).

import { useCallback, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { safeInvoke as invoke } from "../analytics/safeInvoke";
import { useSessionView } from "../session/SessionViewProvider";
import { deferredProviderForLlmStart, useAudioGraphStore } from "../store";
import type { GraphNode, MaterializedNote, ProjectionPatch } from "../types";
import { errorToMessage } from "../utils/errorToMessage";
import Button from "./Button";
import Icon from "./Icon";
import IconButton from "./IconButton";

function byMention(a: GraphNode, b: GraphNode): number {
  return (b.mention_count ?? 0) - (a.mention_count ?? 0);
}

/**
 * A point-in-time fingerprint of the inputs to a synthesis run. We capture it
 * when synthesis succeeds, then compare against the live store to surface a
 * lightweight "may be out of date" hint when the graph or transcript grows.
 */
interface SynthesisResult {
  markdown: string;
  at: number;
  nodeCount: number;
  segmentCount: number;
}

/**
 * Manual whole-session synthesis, extracted from NotesPanel's body (SHELL-R2,
 * plan §R2, ADR-0046) so its trigger can live somewhere other than NotesPanel's own
 * header without forking the state two renderers would otherwise disagree
 * about. In the Capture/During workspace NotesPanel calls this itself
 * (uncontrolled — see the `synthesis` prop below); in the Sessions detail's
 * Notes lens, the detail chrome that owns the "Generate prose summary"
 * overflow item calls it and hands the SAME controller instance to
 * `<NotesPanel synthesis={...} headerActions={false} />` so the result/error
 * NotesPanel renders is the run the overflow item actually triggered.
 */
export interface NotesSynthesisController {
  loading: boolean;
  error: string | null;
  result: SynthesisResult | null;
  handleSynthesize: () => Promise<void>;
  clearError: () => void;
}

export function useNotesSynthesis(): NotesSynthesisController {
  const { t } = useTranslation();
  const { transcriptSegments: segments, graphSnapshot: graph } =
    useSessionView();
  const settings = useAudioGraphStore((s) => s.settings);
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);

  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<SynthesisResult | null>(null);

  const handleSynthesize = useCallback(async () => {
    if (loading) return;
    if (loadedSessionId) {
      setError(t("notes.reviewSynthesisBlocked"));
      return;
    }
    if (!settings) {
      setError(t("errors.providerSettingsLoading"));
      return;
    }
    const deferredProvider = deferredProviderForLlmStart(settings);
    if (deferredProvider) {
      setError(
        errorToMessage({
          code: "provider_deferred",
          message: {
            provider_id: deferredProvider.id,
            display_name: deferredProvider.display_name,
          },
        }),
      );
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const markdown = await invoke<string>("synthesize_notes");
      setResult({
        markdown,
        at: Date.now(),
        nodeCount: graph.nodes?.length ?? 0,
        segmentCount: segments.length,
      });
    } catch (e) {
      setError(errorToMessage(e));
    } finally {
      setLoading(false);
    }
  }, [graph, loadedSessionId, loading, segments, settings, t]);

  return {
    loading,
    error,
    result,
    handleSynthesize,
    clearError: () => setError(null),
  };
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

export default function NotesPanel({
  synthesis: externalSynthesis,
  headerActions = true,
}: NotesPanelProps = {}) {
  const { t, i18n } = useTranslation();
  const {
    transcriptSegments: segments,
    graphSnapshot: graph,
    materializedNotes,
    sessionProjectionEvents: projectionEvents,
  } = useSessionView();
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);
  const loadSampleSessionPreview = useAudioGraphStore(
    (s) => s.loadSampleSessionPreview,
  );

  // Rules-of-hooks-safe: always call the internal controller (mirrors
  // `useSessionView`'s own "unconditionally call, conditionally prefer"
  // convention) and pick whichever the caller actually wants rendered.
  const internalSynthesis = useNotesSynthesis();
  const { loading, error, result, handleSynthesize, clearError } =
    externalSynthesis ?? internalSynthesis;
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

  const liveNotes = materializedNotes?.notes ?? [];
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
    <div
      ref={panelRef}
      className="flex flex-col h-full py-[10px] px-(--space-5) overflow-y-auto"
    >
      <div className="flex items-center justify-between gap-(--space-4) mb-(--space-4)">
        <span className="text-sm font-bold tracking-wide uppercase text-text-secondary">
          <Icon name="notes" size={16} /> {t("notes.title")}
        </span>
        {headerActions && (
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

      {loadedSessionId && (
        <p
          id="notes-review-synthesis-help"
          className="m-0 mb-(--space-4) rounded-sm border border-(--tint-border-warning) bg-(--tint-warning) px-(--space-4) py-(--space-3) text-xs leading-[1.4] text-text-secondary"
          role="status"
        >
          <Icon name="warning" size={14} /> {t("notes.reviewSynthesisBlocked")}
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
          <div className="text-sm leading-[1.5] text-text-primary whitespace-pre-wrap break-words py-(--space-4) px-(--space-5) rounded-lg bg-bg-tertiary border border-(--edge)">
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
              {t("notes.emptyTitle")}
            </p>
            <p className="m-0 text-text-muted text-sm leading-normal">
              {t("notes.empty")}
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
    <li
      data-note-id={note.id}
      className="rounded-md border border-(--edge) bg-bg-tertiary py-(--space-3) px-(--space-4)"
    >
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
      <p className="m-0 mt-(--space-2) text-sm leading-[1.45] text-text-secondary whitespace-pre-wrap [overflow-wrap:anywhere]">
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
            className="text-sm leading-[1.4] text-text-primary [overflow-wrap:anywhere]"
          >
            {n.name}
          </li>
        ))}
      </ul>
    </section>
  );
}
