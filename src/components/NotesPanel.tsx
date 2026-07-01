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
import { invoke } from "@tauri-apps/api/core";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type { GraphNode, MaterializedNote, ProjectionPatch } from "../types";
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

export default function NotesPanel() {
  const { t } = useTranslation();
  const segments = useAudioGraphStore((s) => s.transcriptSegments);
  const graph = useAudioGraphStore((s) => s.graphSnapshot);
  const materializedNotes = useAudioGraphStore((s) => s.materializedNotes);
  const projectionEvents = useAudioGraphStore((s) => s.sessionProjectionEvents);

  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<SynthesisResult | null>(null);

  const handleSynthesize = async () => {
    if (loading) return;
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
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  // Stale when the graph or transcript has grown since the captured snapshot.
  const isStale =
    result !== null &&
    ((graph.nodes?.length ?? 0) !== result.nodeCount ||
      segments.length !== result.segmentCount);

  const synthesizedTime = useMemo(
    () => (result ? new Date(result.at).toLocaleTimeString() : ""),
    [result],
  );

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
    "text-xs font-bold uppercase tracking-[0.5px] text-text-muted mb-[5px]";
  const chipBase =
    "text-sm py-[2px] px-(--space-4) rounded-xl bg-bg-elevated border border-border-color";

  return (
    <div className="flex flex-col h-full py-[10px] px-(--space-5) overflow-y-auto">
      <div className="flex items-center justify-between gap-(--space-4) mb-(--space-4)">
        <span className="text-sm font-bold tracking-[0.4px] uppercase text-text-secondary">
          <Icon name="notes" size={16} /> {t("notes.title")}
        </span>
        <Button
          variant="secondary"
          size="sm"
          icon="refresh"
          loading={loading}
          onClick={handleSynthesize}
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
      </div>

      {error !== null && (
        <div
          role="alert"
          className="flex items-start gap-(--space-3) mb-(--space-4) py-(--space-3) px-(--space-4) rounded-lg bg-(--tint-accent-danger) text-(--text-on-tint-danger) text-sm"
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
            onClick={() => setError(null)}
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
          <div className="text-sm leading-[1.5] text-text-primary whitespace-pre-wrap break-words py-(--space-4) px-(--space-5) rounded-lg bg-bg-tertiary border border-border-color">
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
        <p className="text-text-muted text-sm leading-normal">
          {t("notes.empty")}
        </p>
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
      className="rounded-md border border-border-color bg-bg-tertiary py-(--space-3) px-(--space-4)"
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
              className="text-[11px] py-[1px] px-(--space-2) rounded-sm bg-bg-elevated text-text-muted border border-border-color"
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
  "text-xs font-bold uppercase tracking-[0.5px] text-text-muted mb-[5px]";

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
