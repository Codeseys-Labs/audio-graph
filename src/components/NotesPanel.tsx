/**
 * Notes panel — a structured, readable running summary of the conversation,
 * distinct from the raw transcription log. It is derived purely on the client
 * from existing store state (transcript segments + the typed knowledge graph),
 * so it needs no backend call and updates live.
 *
 * It leans on the conversation ontology (ADR-0008): the graph's typed nodes
 * (Question / Task / Decision / Topic / Person …) become readable sections.
 */
import { useMemo } from "react";
import { useAudioGraphStore } from "../store";
import type { GraphNode } from "../types";
import Icon from "./Icon";

function byMention(a: GraphNode, b: GraphNode): number {
  return (b.mention_count ?? 0) - (a.mention_count ?? 0);
}

export default function NotesPanel() {
  const segments = useAudioGraphStore((s) => s.transcriptSegments);
  const graph = useAudioGraphStore((s) => s.graphSnapshot);

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
      speakers.size > 0
        ? [...speakers]
        : ofType("Person").map((n) => n.name);

    return {
      participants,
      questions: ofType("Question"),
      tasks: ofType("Task"),
      decisions: ofType("Decision"),
      topics: ofType("Topic", "Organization", "Product", "Event"),
    };
  }, [segments, graph]);

  const isEmpty =
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
    "text-sm py-[2px] px-(--space-4) rounded-[10px] bg-bg-elevated border border-border-color";

  return (
    <div className="flex flex-col h-full py-[10px] px-(--space-5)">
      <div className="flex items-center mb-(--space-4)">
        <span className="text-sm font-bold tracking-[0.4px] uppercase text-text-secondary">
          <Icon name="notes" size={16} /> Notes
        </span>
      </div>
      {isEmpty ? (
        <p className="text-text-muted text-sm leading-normal">
          Notes build automatically from the conversation — participants,
          questions, action items, decisions, and key topics will appear here
          as people speak.
        </p>
      ) : (
        <div className="flex flex-col gap-(--space-5)">
          {notes.participants.length > 0 && (
            <section>
              <h4 className={sectionTitle}>Participants</h4>
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
            <NotesList title="Open questions" items={notes.questions} />
          )}
          {notes.tasks.length > 0 && (
            <NotesList title="Action items" items={notes.tasks} />
          )}
          {notes.decisions.length > 0 && (
            <NotesList title="Decisions" items={notes.decisions} />
          )}
          {notes.topics.length > 0 && (
            <section>
              <h4 className={sectionTitle}>Key topics</h4>
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
