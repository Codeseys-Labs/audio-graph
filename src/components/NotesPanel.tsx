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

  return (
    <div className="notes-panel">
      <div className="notes-panel__header">
        <span className="notes-panel__title">📒 Notes</span>
      </div>
      {isEmpty ? (
        <p className="notes-panel__empty">
          Notes build automatically from the conversation — participants,
          questions, action items, decisions, and key topics will appear here
          as people speak.
        </p>
      ) : (
        <div className="notes-panel__body">
          {notes.participants.length > 0 && (
            <section className="notes-section">
              <h4 className="notes-section__title">Participants</h4>
              <div className="notes-chips">
                {notes.participants.map((p) => (
                  <span key={p} className="notes-chip">
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
            <section className="notes-section">
              <h4 className="notes-section__title">Key topics</h4>
              <div className="notes-chips">
                {notes.topics.slice(0, 12).map((n) => (
                  <span key={n.id} className="notes-chip notes-chip--topic">
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

function NotesList({ title, items }: { title: string; items: GraphNode[] }) {
  return (
    <section className="notes-section">
      <h4 className="notes-section__title">{title}</h4>
      <ul className="notes-list">
        {items.slice(0, 8).map((n) => (
          <li key={n.id} className="notes-list__item">
            {n.name}
          </li>
        ))}
      </ul>
    </section>
  );
}
