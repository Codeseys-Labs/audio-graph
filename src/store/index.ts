/**
 * Global frontend store (Zustand) — the single source of truth for UI
 * state plus the invoke-bridge to the Rust backend.
 *
 * Slice layout:
 *   - Audio sources          — `audioSources`, `selectedSourceIds`,
 *                              `searchFilter`, `processes` + fetchers.
 *   - Capture lifecycle      — `isCapturing`, `captureStartTime`,
 *                              `startCapture` / `stopCapture` (wrap
 *                              `start_capture` / `stop_capture`).
 *   - Transcribe pipeline    — `isTranscribing`, `startTranscribe` /
 *                              `stopTranscribe` + the live
 *                              `transcriptSegments` buffer populated by
 *                              `TRANSCRIPT_UPDATE` events.
 *   - Gemini Live            — `isGeminiActive`, `startGemini` /
 *                              `stopGemini`, plus the separate
 *                              `geminiTranscripts` buffer appended by
 *                              `GEMINI_TRANSCRIPTION` events.
 *   - Knowledge graph        — `graphSnapshot` (refreshed on
 *                              `GRAPH_UPDATE`) + the `exportGraph` and
 *                              `getSessionId` command wrappers.
 *   - Speakers               — `speakers` (upserted on
 *                              `SPEAKER_DETECTED` events).
 *   - Pipeline status        — `pipelineStatus` + per-source
 *                              `backpressuredSources` set +
 *                              persistence queue pressure.
 *   - Chat                   — `chatMessages`, `isChatLoading`,
 *                              `sendChatMessage`, `clearChatHistory`.
 *   - Settings / UI          — `settings`, `loadSettings`,
 *                              `settingsOpen` / `sessionsBrowserOpen`
 *                              modal flags + `rightPanelTab` tab state.
 *   - Error + toast wiring   — `error`, `setError`, `clearError`.
 *
 * The invoke-bridge contract: each async action that touches Rust
 * wraps `invoke<T>(command, args)` and translates thrown errors via
 * `utils/errorToMessage`. Events flow the other way — `useTauriEvents`
 * mutates this store on every backend event. See that hook for the
 * full list of subscriptions.
 *
 * Unit tests that exercise slices pull `useAudioGraphStore.getState()`
 * directly and `setState` to seed fixtures.
 */

import { Channel, invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { safeInvoke } from "../analytics/safeInvoke";
import enLocale from "../i18n/locales/en.json";
import ptLocale from "../i18n/locales/pt.json";
import { persistTheme, readStoredTheme } from "../theme";
import type {
  AgentProposalEvent,
  AgentStatusEvent,
  ApiEndpointConfig,
  AppSettings,
  AsrPartialEvent,
  AsrSpanRevisionEvent,
  AudioGraphStore,
  AudioSourceInfo,
  ChatMessage,
  ChatResponse,
  ChatStreamEvent,
  ChatTokenDeltaEvent,
  ChatTokenDoneEvent,
  DiarizationSpanRevisionEvent,
  GeminiTranscriptEntry,
  GraphDelta,
  GraphLink,
  GraphNode,
  GraphSnapshot,
  LiveAssistCardRecord,
  LoadedSession,
  MaterializedGraph,
  MaterializedGraphEdge,
  MaterializedGraphNode,
  MaterializedNote,
  MaterializedNotes,
  ModelInfo,
  ModelStatus,
  PipelineLatencyEvent,
  ProcessedAudioConsumerHealthPayload,
  ProcessInfo,
  ProjectionPatch,
  SessionExportBundle,
  SessionMetadata,
  SessionRecoveryReport,
  StageStatus,
  TranscriptEvent,
  TranscriptSegment,
  TurnLifecycleEvent,
} from "../types";
import { removeExclusiveCapturePeer } from "../utils/captureTarget";
import { errorToMessage } from "../utils/errorToMessage";

const idleStage: StageStatus = { type: "Idle" };

/**
 * Streaming-chat delta coalescing window (audio-graph-1534). Tokens arrive at
 * variable rates from the LLM provider — bursts of 50+ deltas in a single
 * frame are common mid-stream. Without coalescing, every delta would trigger a
 * Zustand subscriber notification + React re-render; at full rate that thrashes
 * layout. The channel `onmessage` handler batches deltas into the store at most
 * once per 33 ms (~30 fps), below the human flicker threshold but well above
 * the burst rate. (Was the `CHAT_DELTA_THROTTLE_MS` coalescer in
 * `useTauriEvents`; it moved here with the transport when the hot path went
 * from `chat-token-delta` events to the per-invocation channel.)
 */
const CHAT_DELTA_THROTTLE_MS = 33;

function upsertLiveAssistCardRecord(
  cards: LiveAssistCardRecord[],
  card: LiveAssistCardRecord,
): LiveAssistCardRecord[] {
  const next = cards.filter(
    (item) =>
      item.session_id !== card.session_id ||
      item.proposal.id !== card.proposal.id,
  );
  next.push(card);
  return next.sort((a, b) => b.updated_at_ms - a.updated_at_ms);
}

function upsertAgentProposal(
  proposals: AgentProposalEvent[],
  proposal: AgentProposalEvent,
): AgentProposalEvent[] {
  return [
    ...proposals.filter((item) => item.id !== proposal.id),
    proposal,
  ].sort((a, b) => b.created_at_ms - a.created_at_ms);
}

function asrRevisionToTranscriptSegment(
  revision: AsrSpanRevisionEvent,
): TranscriptSegment {
  return {
    id: revision.span_id,
    source_id: revision.source_id,
    speaker_id: revision.speaker_id ?? null,
    speaker_label: revision.speaker_label ?? null,
    text: revision.text,
    start_time: revision.start_time,
    end_time: revision.end_time,
    confidence: revision.confidence,
  };
}

function asrRevisionToTranscriptEvent(
  revision: AsrSpanRevisionEvent,
): TranscriptEvent {
  return {
    span_id: revision.span_id,
    provider: revision.provider,
    source_id: revision.source_id,
    provider_item_id: revision.provider_item_id ?? null,
    transcript_segment_id: revision.transcript_segment_id ?? null,
    speaker_id: revision.speaker_id ?? null,
    speaker_label: revision.speaker_label ?? null,
    channel: revision.channel ?? null,
    text: revision.text,
    start_time: revision.start_time,
    end_time: revision.end_time,
    confidence: revision.confidence,
    is_final: revision.is_final,
    stability: revision.stability,
    revision_number: revision.revision_number,
    supersedes: revision.supersedes ?? null,
    turn_id: revision.turn_id ?? null,
    end_of_turn: revision.end_of_turn,
    raw_event_ref: revision.raw_event_ref ?? null,
    capture_latency_ms: revision.capture_latency_ms ?? null,
    asr_latency_ms: revision.asr_latency_ms ?? null,
    received_at_ms: revision.received_at_ms,
  };
}

function asrRevisionSegmentKeys(revision: AsrSpanRevisionEvent): Set<string> {
  return new Set(
    [revision.span_id, revision.transcript_segment_id].filter(
      (id): id is string => Boolean(id),
    ),
  );
}

function isStaleAsrRevision(
  revisions: AsrSpanRevisionEvent[],
  revision: AsrSpanRevisionEvent,
): boolean {
  return revisions.some(
    (candidate) =>
      candidate.span_id === revision.span_id &&
      candidate.revision_number >= revision.revision_number,
  );
}

function applyAsrRevisionToTranscriptSegments(
  segments: TranscriptSegment[],
  revisions: AsrSpanRevisionEvent[],
  revision: AsrSpanRevisionEvent,
): TranscriptSegment[] {
  if (isStaleAsrRevision(revisions, revision)) return segments;

  const nextSegment = asrRevisionToTranscriptSegment(revision);
  const keys = asrRevisionSegmentKeys(revision);
  const replaceIndex = segments.findIndex((segment) => keys.has(segment.id));
  if (replaceIndex < 0) {
    return [...segments.slice(-499), nextSegment];
  }

  const withoutMatching = segments.filter((segment) => !keys.has(segment.id));
  withoutMatching.splice(
    Math.min(replaceIndex, withoutMatching.length),
    0,
    nextSegment,
  );
  return withoutMatching;
}

function projectionPatchNote(
  patch: ProjectionPatch,
  id: string,
  title: string,
  body: string,
  tags: string[],
): MaterializedNote {
  return {
    id,
    title,
    body,
    tags,
    updated_by_sequence: patch.sequence,
    updated_at_ms: patch.created_at_ms,
    basis: patch.basis,
    provenance: patch.provenance,
  };
}

function applyProjectionNotesPatch(
  current: MaterializedNotes | null,
  patch: ProjectionPatch,
): MaterializedNotes | null {
  if (patch.kind !== "notes") return current;
  if (current && patch.sequence <= current.last_sequence) return current;

  const notes: MaterializedNotes = current
    ? {
        ...current,
        notes: current.notes.map((note) => ({
          ...note,
          tags: [...note.tags],
        })),
      }
    : {
        schema_version: 1,
        session_id: "live",
        last_sequence: 0,
        notes: [],
      };

  for (const operation of patch.operations) {
    switch (operation.type) {
      case "upsert_note": {
        const next = projectionPatchNote(
          patch,
          operation.id,
          operation.title,
          operation.body,
          operation.tags,
        );
        const index = notes.notes.findIndex((note) => note.id === operation.id);
        if (index >= 0) notes.notes[index] = next;
        else notes.notes.push(next);
        break;
      }
      case "delete_note":
        notes.notes = notes.notes.filter((note) => note.id !== operation.id);
        break;
      case "reorder_note": {
        const fromIndex = notes.notes.findIndex(
          (note) => note.id === operation.id,
        );
        if (fromIndex < 0 || operation.after_id === operation.id) break;
        const [note] = notes.notes.splice(fromIndex, 1);
        if (operation.after_id == null) {
          notes.notes.unshift(note);
          break;
        }
        const afterIndex = notes.notes.findIndex(
          (candidate) => candidate.id === operation.after_id,
        );
        if (afterIndex < 0) {
          notes.notes.splice(fromIndex, 0, note);
          break;
        }
        notes.notes.splice(afterIndex + 1, 0, note);
        break;
      }
      default:
        break;
    }
  }

  notes.last_sequence = patch.sequence;
  return notes;
}

function projectionGraphPatchNode(
  patch: ProjectionPatch,
  id: string,
  name: string,
  entityType: string,
  description?: string | null,
): MaterializedGraphNode {
  return {
    id,
    name,
    entity_type: entityType,
    description: description ?? null,
    confidence: patch.confidence,
    valid_from_ms: patch.created_at_ms,
    valid_until_ms: null,
    updated_by_sequence: patch.sequence,
    updated_at_ms: patch.created_at_ms,
    basis: patch.basis,
    provenance: patch.provenance,
  };
}

function projectionGraphPatchEdge(
  patch: ProjectionPatch,
  id: string,
  source: string,
  target: string,
  relationType: string,
  label: string | null | undefined,
  weight: number,
): MaterializedGraphEdge {
  return {
    id,
    source,
    target,
    relation_type: relationType,
    label: label ?? null,
    weight,
    confidence: patch.confidence,
    valid_from_ms: patch.created_at_ms,
    valid_until_ms: null,
    updated_by_sequence: patch.sequence,
    updated_at_ms: patch.created_at_ms,
    basis: patch.basis,
    provenance: patch.provenance,
  };
}

function activeMaterializedNode(graph: MaterializedGraph, id: string): boolean {
  return graph.nodes.some(
    (node) => node.id === id && node.valid_until_ms == null,
  );
}

function activeMaterializedNodeIndex(
  graph: MaterializedGraph,
  id: string,
): number {
  return graph.nodes.findIndex(
    (node) => node.id === id && node.valid_until_ms == null,
  );
}

function activeMaterializedEdgeIndex(
  graph: MaterializedGraph,
  id: string,
): number {
  return graph.edges.findIndex(
    (edge) => edge.id === id && edge.valid_until_ms == null,
  );
}

function invalidateMaterializedNodeAt(
  graph: MaterializedGraph,
  index: number,
  patch: ProjectionPatch,
): void {
  const node = graph.nodes[index];
  graph.nodes[index] = {
    ...node,
    confidence: patch.confidence,
    valid_until_ms: patch.created_at_ms,
    updated_by_sequence: patch.sequence,
    updated_at_ms: patch.created_at_ms,
    basis: patch.basis,
    provenance: patch.provenance,
  };
}

function invalidateMaterializedEdgeAt(
  graph: MaterializedGraph,
  index: number,
  patch: ProjectionPatch,
): void {
  const edge = graph.edges[index];
  graph.edges[index] = {
    ...edge,
    confidence: patch.confidence,
    valid_until_ms: patch.created_at_ms,
    updated_by_sequence: patch.sequence,
    updated_at_ms: patch.created_at_ms,
    basis: patch.basis,
    provenance: patch.provenance,
  };
}

function cleanupDuplicateActiveMaterializedEdges(
  graph: MaterializedGraph,
  patch: ProjectionPatch,
): void {
  const winners = new Map<string, number>();
  for (let index = 0; index < graph.edges.length; index += 1) {
    const edge = graph.edges[index];
    if (edge.valid_until_ms != null) continue;
    const key = `${edge.source}\u0000${edge.target}\u0000${edge.relation_type}`;
    const winnerIndex = winners.get(key);
    if (winnerIndex == null) {
      winners.set(key, index);
      continue;
    }

    const winner = graph.edges[winnerIndex];
    graph.edges[winnerIndex] = {
      ...winner,
      weight: Math.max(winner.weight, edge.weight),
      label: winner.label ?? edge.label,
      confidence: Math.max(winner.confidence, edge.confidence),
      updated_by_sequence: patch.sequence,
      updated_at_ms: patch.created_at_ms,
      basis: patch.basis,
      provenance: patch.provenance,
    };
    invalidateMaterializedEdgeAt(graph, index, patch);
  }
}

function applyProjectionGraphPatch(
  current: MaterializedGraph | null,
  patch: ProjectionPatch,
): MaterializedGraph | null {
  if (patch.kind !== "graph") return current;
  if (current && patch.sequence <= current.last_sequence) return current;

  const graph: MaterializedGraph = current
    ? {
        ...current,
        nodes: current.nodes.map((node) => ({ ...node })),
        edges: current.edges.map((edge) => ({ ...edge })),
      }
    : {
        schema_version: 1,
        session_id: "live",
        last_sequence: 0,
        nodes: [],
        edges: [],
      };

  for (const operation of patch.operations) {
    switch (operation.type) {
      case "upsert_graph_node": {
        const next = projectionGraphPatchNode(
          patch,
          operation.id,
          operation.name,
          operation.entity_type,
          operation.description,
        );
        const index = graph.nodes.findIndex((node) => node.id === operation.id);
        if (index >= 0) graph.nodes[index] = next;
        else graph.nodes.push(next);
        break;
      }
      case "remove_graph_node":
        graph.nodes = graph.nodes.filter((node) => node.id !== operation.id);
        graph.edges = graph.edges.filter(
          (edge) =>
            edge.source !== operation.id && edge.target !== operation.id,
        );
        break;
      case "invalidate_graph_node": {
        const index = activeMaterializedNodeIndex(graph, operation.id);
        if (index < 0) break;
        invalidateMaterializedNodeAt(graph, index, patch);
        for (
          let edgeIndex = 0;
          edgeIndex < graph.edges.length;
          edgeIndex += 1
        ) {
          const edge = graph.edges[edgeIndex];
          if (
            edge.valid_until_ms == null &&
            (edge.source === operation.id || edge.target === operation.id)
          ) {
            invalidateMaterializedEdgeAt(graph, edgeIndex, patch);
          }
        }
        break;
      }
      case "upsert_graph_edge": {
        if (
          !activeMaterializedNode(graph, operation.source) ||
          !activeMaterializedNode(graph, operation.target)
        ) {
          break;
        }
        const next = projectionGraphPatchEdge(
          patch,
          operation.id,
          operation.source,
          operation.target,
          operation.relation_type,
          operation.label,
          operation.weight,
        );
        const index = graph.edges.findIndex((edge) => edge.id === operation.id);
        if (index >= 0) graph.edges[index] = next;
        else graph.edges.push(next);
        break;
      }
      case "remove_graph_edge":
        graph.edges = graph.edges.filter((edge) => edge.id !== operation.id);
        break;
      case "invalidate_graph_edge": {
        const index = activeMaterializedEdgeIndex(graph, operation.id);
        if (index >= 0) invalidateMaterializedEdgeAt(graph, index, patch);
        break;
      }
      case "strengthen_graph_edge":
      case "weaken_graph_edge": {
        const index = activeMaterializedEdgeIndex(graph, operation.id);
        if (index < 0 || !Number.isFinite(operation.weight_delta)) break;
        const sign = operation.type === "strengthen_graph_edge" ? 1 : -1;
        const edge = graph.edges[index];
        graph.edges[index] = {
          ...edge,
          weight: Math.max(
            0,
            Math.min(1, edge.weight + sign * operation.weight_delta),
          ),
          confidence: patch.confidence,
          updated_by_sequence: patch.sequence,
          updated_at_ms: patch.created_at_ms,
          basis: patch.basis,
          provenance: patch.provenance,
        };
        break;
      }
      case "merge_graph_nodes": {
        if (
          operation.source_id === operation.target_id ||
          !activeMaterializedNode(graph, operation.target_id)
        ) {
          break;
        }
        const sourceIndex = activeMaterializedNodeIndex(
          graph,
          operation.source_id,
        );
        if (sourceIndex < 0) break;
        invalidateMaterializedNodeAt(graph, sourceIndex, patch);
        for (let index = 0; index < graph.edges.length; index += 1) {
          const edge = graph.edges[index];
          if (edge.valid_until_ms != null) continue;
          let next = edge;
          if (next.source === operation.source_id) {
            next = { ...next, source: operation.target_id };
          }
          if (next.target === operation.source_id) {
            next = { ...next, target: operation.target_id };
          }
          if (next.source === next.target) {
            graph.edges[index] = next;
            invalidateMaterializedEdgeAt(graph, index, patch);
          } else if (
            next.source === operation.target_id ||
            next.target === operation.target_id
          ) {
            graph.edges[index] = {
              ...next,
              updated_by_sequence: patch.sequence,
              updated_at_ms: patch.created_at_ms,
              basis: patch.basis,
              provenance: patch.provenance,
            };
          }
        }
        cleanupDuplicateActiveMaterializedEdges(graph, patch);
        break;
      }
      case "split_graph_node": {
        if (operation.replacement_nodes.length < 2) break;
        const index = activeMaterializedNodeIndex(graph, operation.id);
        if (index < 0) break;
        invalidateMaterializedNodeAt(graph, index, patch);
        for (
          let edgeIndex = 0;
          edgeIndex < graph.edges.length;
          edgeIndex += 1
        ) {
          const edge = graph.edges[edgeIndex];
          if (
            edge.valid_until_ms == null &&
            (edge.source === operation.id || edge.target === operation.id)
          ) {
            invalidateMaterializedEdgeAt(graph, edgeIndex, patch);
          }
        }
        for (const replacement of operation.replacement_nodes) {
          const next = projectionGraphPatchNode(
            patch,
            replacement.id,
            replacement.name,
            replacement.entity_type,
            replacement.description,
          );
          const replacementIndex = graph.nodes.findIndex(
            (node) => node.id === replacement.id,
          );
          if (replacementIndex >= 0) graph.nodes[replacementIndex] = next;
          else graph.nodes.push(next);
        }
        break;
      }
      default:
        break;
    }
  }

  graph.last_sequence = patch.sequence;
  return graph;
}

function graphEndpointId(endpoint: GraphLink["source"]): string {
  return typeof endpoint === "string" ? endpoint : endpoint.id;
}

function graphLinkId(link: GraphLink): string {
  return (
    link.id ??
    `${graphEndpointId(link.source)}->${graphEndpointId(link.target)}:${link.relation_type}:${link.label ?? ""}`
  );
}

// react-force-graph stores live x/y on the node objects. New nodes arrive
// without coordinates and default to (0,0) — when several appear at once they
// pile on the origin and every edge fans to that single point (the "jank").
// Seed each new node near an already-positioned neighbour (or near the centre
// with jitter) so it enters the layout in a sensible place instead of origin.
type PositionedNode = GraphNode & { x?: number; y?: number };
function seedNodePositions(
  nextNodes: GraphNode[],
  links: GraphLink[],
  positioned: Map<string, PositionedNode>,
) {
  const adjacency = new Map<string, string[]>();
  const adjacencyFor = (id: string): string[] => {
    let list = adjacency.get(id);
    if (!list) {
      list = [];
      adjacency.set(id, list);
    }
    return list;
  };
  for (const link of links) {
    const s = graphEndpointId(link.source);
    const t = graphEndpointId(link.target);
    adjacencyFor(s).push(t);
    adjacencyFor(t).push(s);
  }
  for (const node of nextNodes as PositionedNode[]) {
    if (typeof node.x === "number" && typeof node.y === "number") continue;
    let seeded = false;
    for (const neighborId of adjacency.get(node.id) ?? []) {
      const nb = positioned.get(neighborId);
      if (nb && typeof nb.x === "number" && typeof nb.y === "number") {
        node.x = nb.x + (Math.random() - 0.5) * 40;
        node.y = nb.y + (Math.random() - 0.5) * 40;
        seeded = true;
        break;
      }
    }
    if (!seeded) {
      // Spread around the centre rather than stacking on (0,0).
      const angle = Math.random() * Math.PI * 2;
      const radius = 60 + Math.random() * 120;
      node.x = Math.cos(angle) * radius;
      node.y = Math.sin(angle) * radius;
    }
  }
}

const SAMPLE_SESSION_ID = "sample-session-preview";
const SAMPLE_PREVIEW_BASE_MS = 1_700_000_000_000;
const SAMPLE_PREVIEW_CATALOG = {
  en: enLocale.samplePreview,
  pt: ptLocale.samplePreview,
} as const;

type SamplePreviewLocale = keyof typeof SAMPLE_PREVIEW_CATALOG;

function emptyGraphSnapshot(): GraphSnapshot {
  return {
    nodes: [],
    links: [],
    stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
  };
}

function samplePreviewLocale(language?: string): SamplePreviewLocale {
  const normalized = language?.toLowerCase().split(/[-_]/)[0];
  return normalized === "pt" ? "pt" : "en";
}

function samplePreviewCopy(language?: string) {
  return SAMPLE_PREVIEW_CATALOG[samplePreviewLocale(language)];
}

function clearSamplePreviewState() {
  return {
    samplePreviewActive: false,
    transcriptSegments: [],
    asrPartial: null,
    asrSpanRevisions: [],
    diarizationSpanRevisions: [],
    sessionTranscriptEvents: [],
    sessionProjectionEvents: [],
    materializedNotes: null,
    materializedProjectionGraph: null,
    turnEvents: [],
    agentStatus: null,
    agentProposals: [],
    liveAssistCards: [],
    approvingAgentProposalIds: [],
    graphSnapshot: emptyGraphSnapshot(),
    speakers: [],
  };
}

function exitSamplePreviewState(active: boolean) {
  return active ? clearSamplePreviewState() : { samplePreviewActive: false };
}

function sampleSessionPreviewState(language?: string) {
  const copy = samplePreviewCopy(language);
  const sourceId = "sample-source";
  const provenance = {
    source: "built_in_sample_preview",
    model: "sample-session-v1",
    prompt_id: "sample_session_preview_v1",
  };
  const basis = {
    session_id: SAMPLE_SESSION_ID,
    transcript_span_ids: [
      "sample-span-1",
      "sample-span-2",
      "sample-span-3",
      "sample-span-4",
    ],
  };
  const transcriptSegments: TranscriptSegment[] = [
    {
      id: "sample-segment-1",
      source_id: sourceId,
      speaker_id: "sample-speaker-1",
      speaker_label: copy.speakers.host,
      text: copy.transcript.setupCredential,
      start_time: 0,
      end_time: 4.2,
      confidence: 0.96,
    },
    {
      id: "sample-segment-2",
      source_id: sourceId,
      speaker_id: "sample-speaker-2",
      speaker_label: copy.speakers.engineer,
      text: copy.transcript.firstRun,
      start_time: 4.4,
      end_time: 9.1,
      confidence: 0.94,
    },
    {
      id: "sample-segment-3",
      source_id: sourceId,
      speaker_id: "sample-speaker-1",
      speaker_label: copy.speakers.host,
      text: copy.transcript.revisionGraph,
      start_time: 9.4,
      end_time: 14.7,
      confidence: 0.95,
    },
    {
      id: "sample-segment-4",
      source_id: sourceId,
      speaker_id: "sample-speaker-2",
      speaker_label: copy.speakers.engineer,
      text: copy.transcript.platformRelease,
      start_time: 15.1,
      end_time: 19.2,
      confidence: 0.93,
    },
  ];

  const asrSpanRevisions: AsrSpanRevisionEvent[] = transcriptSegments.map(
    (segment, index) => ({
      span_id: `sample-span-${index + 1}`,
      provider: "sample",
      source_id: segment.source_id,
      provider_item_id: `sample-provider-turn-${index + 1}`,
      transcript_segment_id: segment.id,
      speaker_id: segment.speaker_id,
      speaker_label: segment.speaker_label,
      channel: null,
      text: segment.text,
      start_time: segment.start_time,
      end_time: segment.end_time,
      confidence: segment.confidence,
      is_final: true,
      stability: "final",
      revision_number: 1,
      supersedes: null,
      turn_id: `sample-turn-${index + 1}`,
      end_of_turn: true,
      raw_event_ref: `sample.turn.${index + 1}`,
      capture_latency_ms: 42,
      asr_latency_ms: 180 + index * 20,
      received_at_ms: SAMPLE_PREVIEW_BASE_MS + index * 1_000,
    }),
  );

  const sessionTranscriptEvents = asrSpanRevisions.map(
    asrRevisionToTranscriptEvent,
  );

  const sessionProjectionEvents: ProjectionPatch[] = [
    {
      sequence: 1,
      kind: "notes",
      llm_request_id: "sample-notes-1",
      basis,
      operations: [
        {
          type: "upsert_note",
          id: "sample-note-setup",
          title: copy.notes.setupTitle,
          body: copy.notes.setupBody,
          tags: [...copy.notes.setupTags],
        },
        {
          type: "upsert_note",
          id: "sample-note-retcon",
          title: copy.notes.retconTitle,
          body: copy.notes.retconBody,
          tags: [...copy.notes.retconTags],
        },
        {
          type: "upsert_note",
          id: "sample-note-platform",
          title: copy.notes.platformTitle,
          body: copy.notes.platformBody,
          tags: [...copy.notes.platformTags],
        },
      ],
      confidence: 0.9,
      provenance,
      queued_at_ms: SAMPLE_PREVIEW_BASE_MS + 2_200,
      generation_latency_ms: 740,
      apply_latency_ms: 24,
      created_at_ms: SAMPLE_PREVIEW_BASE_MS + 3_000,
    },
    {
      sequence: 2,
      kind: "graph",
      llm_request_id: "sample-graph-1",
      basis,
      operations: [
        {
          type: "upsert_graph_node",
          id: "sample-topic-setup",
          name: copy.graph.savedCredentialsName,
          entity_type: "Topic",
          description: copy.graph.savedCredentialsDescription,
        },
        {
          type: "upsert_graph_node",
          id: "sample-decision-retcon",
          name: copy.graph.retconDecisionName,
          entity_type: "Decision",
          description: copy.graph.retconDecisionDescription,
        },
        {
          type: "upsert_graph_node",
          id: "sample-task-release",
          name: copy.graph.releaseTaskName,
          entity_type: "Task",
          description: copy.graph.releaseTaskDescription,
        },
        {
          type: "upsert_graph_node",
          id: "sample-question-provider",
          name: copy.graph.providerQuestionName,
          entity_type: "Question",
          description: copy.graph.providerQuestionDescription,
        },
        {
          type: "upsert_graph_edge",
          id: "sample-edge-setup-provider",
          source: "sample-topic-setup",
          target: "sample-question-provider",
          relation_type: "raises",
          label: copy.graph.raisesLabel,
          weight: 0.74,
        },
        {
          type: "upsert_graph_edge",
          id: "sample-edge-retcon-release",
          source: "sample-decision-retcon",
          target: "sample-task-release",
          relation_type: "tracks",
          label: copy.graph.tracksLabel,
          weight: 0.68,
        },
      ],
      confidence: 0.88,
      provenance,
      queued_at_ms: SAMPLE_PREVIEW_BASE_MS + 2_900,
      generation_latency_ms: 810,
      apply_latency_ms: 31,
      created_at_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
    },
  ];

  const materializedNotes: MaterializedNotes = {
    schema_version: 1,
    session_id: SAMPLE_SESSION_ID,
    last_sequence: 1,
    notes: [
      {
        id: "sample-note-setup",
        title: copy.notes.setupTitle,
        body: copy.notes.setupBody,
        tags: [...copy.notes.setupTags],
        updated_by_sequence: 1,
        updated_at_ms: SAMPLE_PREVIEW_BASE_MS + 3_000,
        basis,
        provenance,
      },
      {
        id: "sample-note-retcon",
        title: copy.notes.retconTitle,
        body: copy.notes.retconBody,
        tags: [...copy.notes.retconTags],
        updated_by_sequence: 1,
        updated_at_ms: SAMPLE_PREVIEW_BASE_MS + 3_000,
        basis,
        provenance,
      },
      {
        id: "sample-note-platform",
        title: copy.notes.platformTitle,
        body: copy.notes.platformBody,
        tags: [...copy.notes.platformTags],
        updated_by_sequence: 1,
        updated_at_ms: SAMPLE_PREVIEW_BASE_MS + 3_000,
        basis,
        provenance,
      },
    ],
  };

  const materializedProjectionGraph: MaterializedGraph = {
    schema_version: 1,
    session_id: SAMPLE_SESSION_ID,
    last_sequence: 2,
    nodes: [
      {
        id: "sample-topic-setup",
        name: copy.graph.savedCredentialsName,
        entity_type: "Topic",
        description: copy.graph.savedCredentialsDescription,
        confidence: 0.9,
        valid_from_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        valid_until_ms: null,
        updated_by_sequence: 2,
        updated_at_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        basis,
        provenance,
      },
      {
        id: "sample-decision-retcon",
        name: copy.graph.retconDecisionName,
        entity_type: "Decision",
        description: copy.graph.retconDecisionDescription,
        confidence: 0.88,
        valid_from_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        valid_until_ms: null,
        updated_by_sequence: 2,
        updated_at_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        basis,
        provenance,
      },
      {
        id: "sample-task-release",
        name: copy.graph.releaseTaskName,
        entity_type: "Task",
        description: copy.graph.releaseTaskDescription,
        confidence: 0.86,
        valid_from_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        valid_until_ms: null,
        updated_by_sequence: 2,
        updated_at_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        basis,
        provenance,
      },
      {
        id: "sample-question-provider",
        name: copy.graph.providerQuestionName,
        entity_type: "Question",
        description: copy.graph.providerQuestionDescription,
        confidence: 0.82,
        valid_from_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        valid_until_ms: null,
        updated_by_sequence: 2,
        updated_at_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        basis,
        provenance,
      },
    ],
    edges: [
      {
        id: "sample-edge-setup-provider",
        source: "sample-topic-setup",
        target: "sample-question-provider",
        relation_type: "raises",
        label: copy.graph.raisesLabel,
        weight: 0.74,
        confidence: 0.88,
        valid_from_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        valid_until_ms: null,
        updated_by_sequence: 2,
        updated_at_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        basis,
        provenance,
      },
      {
        id: "sample-edge-retcon-release",
        source: "sample-decision-retcon",
        target: "sample-task-release",
        relation_type: "tracks",
        label: copy.graph.tracksLabel,
        weight: 0.68,
        confidence: 0.88,
        valid_from_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        valid_until_ms: null,
        updated_by_sequence: 2,
        updated_at_ms: SAMPLE_PREVIEW_BASE_MS + 4_000,
        basis,
        provenance,
      },
    ],
  };

  const graphSnapshot = {
    nodes: [
      {
        id: "sample-topic-setup",
        name: copy.graph.savedCredentialsName,
        entity_type: "Topic",
        val: 3,
        color: "#f472b6",
        first_seen: 0,
        last_seen: 19.2,
        mention_count: 2,
        description: copy.graph.savedCredentialsShortDescription,
      },
      {
        id: "sample-decision-retcon",
        name: copy.graph.retconDecisionName,
        entity_type: "Decision",
        val: 3,
        color: "#94a3b8",
        first_seen: 9.4,
        last_seen: 14.7,
        mention_count: 1,
        description: copy.graph.retconDecisionShortDescription,
      },
      {
        id: "sample-task-release",
        name: copy.graph.releaseTaskName,
        entity_type: "Task",
        val: 2,
        color: "#94a3b8",
        first_seen: 15.1,
        last_seen: 19.2,
        mention_count: 1,
        description: copy.graph.releaseTaskDescription,
      },
      {
        id: "sample-question-provider",
        name: copy.graph.providerQuestionName,
        entity_type: "Question",
        val: 2,
        color: "#94a3b8",
        first_seen: 4.4,
        last_seen: 9.1,
        mention_count: 1,
        description: copy.graph.providerQuestionShortDescription,
      },
    ],
    links: [
      {
        id: "sample-edge-setup-provider",
        source: "sample-topic-setup",
        target: "sample-question-provider",
        relation_type: "raises",
        weight: 0.74,
        color: "#94a3b8",
        label: copy.graph.raisesLabel,
      },
      {
        id: "sample-edge-retcon-release",
        source: "sample-decision-retcon",
        target: "sample-task-release",
        relation_type: "tracks",
        weight: 0.68,
        color: "#a78bfa",
        label: copy.graph.tracksLabel,
      },
    ],
    stats: { total_nodes: 4, total_edges: 2, total_episodes: 1 },
  };

  const pendingProposal: AgentProposalEvent = {
    id: "sample-live-assist-question",
    source_segment_id: "sample-segment-2",
    source_id: sourceId,
    speaker_label: copy.speakers.engineer,
    kind: "question",
    title: copy.liveAssist.questionTitle,
    body: copy.liveAssist.questionBody,
    confidence: 0.84,
    created_at_ms: SAMPLE_PREVIEW_BASE_MS + 5_000,
  };

  return {
    samplePreviewActive: true,
    transcriptSegments,
    asrPartial: null,
    asrSpanRevisions,
    diarizationSpanRevisions: [],
    sessionTranscriptEvents,
    sessionProjectionEvents,
    materializedNotes,
    materializedProjectionGraph,
    turnEvents: [],
    agentStatus: null,
    agentProposals: [],
    liveAssistCards: [
      {
        session_id: SAMPLE_SESSION_ID,
        proposal: pendingProposal,
        status: "pending" as const,
        source_span_ids: [pendingProposal.source_segment_id],
        graph_context_ids: ["sample-topic-setup", "sample-question-provider"],
        outcome: null,
        projection_patch_sequence: null,
        created_at_ms: pendingProposal.created_at_ms,
        updated_at_ms: pendingProposal.created_at_ms,
      },
      {
        session_id: SAMPLE_SESSION_ID,
        proposal: {
          id: "sample-live-assist-note",
          source_segment_id: "sample-segment-3",
          source_id: sourceId,
          speaker_label: copy.speakers.host,
          kind: "note" as const,
          title: copy.liveAssist.noteTitle,
          body: copy.liveAssist.noteBody,
          confidence: 0.87,
          created_at_ms: SAMPLE_PREVIEW_BASE_MS + 5_500,
        },
        status: "approved" as const,
        source_span_ids: ["sample-segment-3"],
        graph_context_ids: ["sample-decision-retcon"],
        outcome: {
          proposal_id: "sample-live-assist-note",
          action: "preview_only",
          message: copy.liveAssist.approvedMessage,
          graph_updated: false,
          timestamp_ms: SAMPLE_PREVIEW_BASE_MS + 5_700,
        },
        projection_patch_sequence: 2,
        created_at_ms: SAMPLE_PREVIEW_BASE_MS + 5_500,
        updated_at_ms: SAMPLE_PREVIEW_BASE_MS + 5_700,
      },
    ],
    approvingAgentProposalIds: [],
    graphSnapshot,
    speakers: [
      {
        id: "sample-speaker-1",
        label: copy.speakers.host,
        color: "#60a5fa",
        total_speaking_time: 9.5,
        segment_count: 2,
      },
      {
        id: "sample-speaker-2",
        label: copy.speakers.engineer,
        color: "#f59e0b",
        total_speaking_time: 8.8,
        segment_count: 2,
      },
    ],
    isCapturing: false,
    isTranscribing: false,
    isGeminiActive: false,
    activeGeminiCommand: null,
    captureStartTime: null,
    backpressuredSources: [],
    persistenceQueueBackpressure: {},
    rightPanelTab: "transcript" as const,
    agentOverlayOpen: true,
    tokenOverlayOpen: false,
    error: null,
  };
}

export const useAudioGraphStore = create<AudioGraphStore>((set, get) => ({
  // ── Audio sources ────────────────────────────────────────────────────
  audioSources: [],
  selectedSourceIds: [],
  setAudioSources: (sources) => set({ audioSources: sources }),
  toggleSourceId: (id) =>
    set((state) => {
      const idx = state.selectedSourceIds.indexOf(id);
      if (idx >= 0) {
        return {
          selectedSourceIds: state.selectedSourceIds.filter(
            (sid) => sid !== id,
          ),
        };
      }
      return {
        selectedSourceIds: [
          ...removeExclusiveCapturePeer(state.selectedSourceIds, id),
          id,
        ],
      };
    }),
  removeSelectedSourceIds: (ids) =>
    set((state) => {
      const idsToRemove = new Set(ids);
      return {
        selectedSourceIds: state.selectedSourceIds.filter(
          (sourceId) => !idsToRemove.has(sourceId),
        ),
      };
    }),
  clearSelectedSources: () => set({ selectedSourceIds: [] }),
  sourceRecoveryIntent: null,
  requestSourceRecovery: (intent) =>
    set((state) => ({
      sourceRecoveryIntent: {
        ...intent,
        id: (state.sourceRecoveryIntent?.id ?? 0) + 1,
        requestedAt: Date.now(),
      },
    })),
  clearSourceRecoveryIntent: () => set({ sourceRecoveryIntent: null }),
  fetchSources: async () => {
    try {
      const sources = await safeInvoke<AudioSourceInfo[]>("list_audio_sources");
      set({ audioSources: sources, error: null });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },

  // ── Processes ────────────────────────────────────────────────────────
  processes: [],
  searchFilter: "",
  fetchProcesses: async () => {
    try {
      const processes = await safeInvoke<ProcessInfo[]>(
        "list_running_processes",
      );
      set({ processes });
    } catch (err) {
      console.error("Failed to fetch processes:", err);
    }
  },
  setSearchFilter: (filter: string) => set({ searchFilter: filter }),

  // ── Transcript ───────────────────────────────────────────────────────
  samplePreviewActive: false,
  transcriptSegments: [],
  asrPartial: null,
  asrSpanRevisions: [],
  diarizationSpanRevisions: [],
  sessionTranscriptEvents: [],
  sessionProjectionEvents: [],
  materializedNotes: null,
  materializedProjectionGraph: null,
  turnEvents: [],
  agentStatus: null,
  agentProposals: [],
  liveAssistCards: [],
  approvingAgentProposalIds: [],
  addTranscriptSegment: (segment) =>
    set((state) => {
      const transcriptSegments = state.samplePreviewActive
        ? []
        : state.transcriptSegments;
      return {
        ...exitSamplePreviewState(state.samplePreviewActive),
        transcriptSegments: [...transcriptSegments.slice(-499), segment],
        asrPartial: null,
      };
    }),
  setAsrPartial: (partial: AsrPartialEvent | null) =>
    set((state) => ({
      ...(partial ? exitSamplePreviewState(state.samplePreviewActive) : {}),
      asrPartial: partial,
    })),
  addAsrSpanRevision: (revision: AsrSpanRevisionEvent) =>
    set((state) => {
      const existingSegments = state.samplePreviewActive
        ? []
        : state.transcriptSegments;
      const existingRevisions = state.samplePreviewActive
        ? []
        : state.asrSpanRevisions;
      const existingEvents = state.samplePreviewActive
        ? []
        : state.sessionTranscriptEvents;
      const transcriptSegments = applyAsrRevisionToTranscriptSegments(
        existingSegments,
        existingRevisions,
        revision,
      );
      return {
        ...exitSamplePreviewState(state.samplePreviewActive),
        asrSpanRevisions: [...existingRevisions.slice(-499), revision],
        sessionTranscriptEvents: [
          ...existingEvents,
          asrRevisionToTranscriptEvent(revision),
        ],
        transcriptSegments,
        asrPartial:
          transcriptSegments === existingSegments ? state.asrPartial : null,
      };
    }),
  addDiarizationSpanRevision: (revision: DiarizationSpanRevisionEvent) =>
    set((state) => {
      const existing = state.samplePreviewActive
        ? []
        : state.diarizationSpanRevisions;
      return {
        ...exitSamplePreviewState(state.samplePreviewActive),
        diarizationSpanRevisions: [...existing.slice(-499), revision],
      };
    }),
  addTurnEvent: (event: TurnLifecycleEvent) =>
    set((state) => {
      const existing = state.samplePreviewActive ? [] : state.turnEvents;
      return {
        ...exitSamplePreviewState(state.samplePreviewActive),
        turnEvents: [...existing.slice(-99), event],
      };
    }),
  addProjectionPatch: (patch: ProjectionPatch) =>
    set((state) => ({
      ...exitSamplePreviewState(state.samplePreviewActive),
      sessionProjectionEvents: [
        ...(state.samplePreviewActive ? [] : state.sessionProjectionEvents),
        patch,
      ],
      materializedNotes: applyProjectionNotesPatch(
        state.samplePreviewActive ? null : state.materializedNotes,
        patch,
      ),
      materializedProjectionGraph: applyProjectionGraphPatch(
        state.samplePreviewActive ? null : state.materializedProjectionGraph,
        patch,
      ),
    })),
  setMaterializedNotes: (notes: MaterializedNotes) =>
    set((state) => ({
      ...exitSamplePreviewState(state.samplePreviewActive),
      materializedNotes: notes,
    })),
  setMaterializedProjectionGraph: (graph: MaterializedGraph) =>
    set((state) => ({
      ...exitSamplePreviewState(state.samplePreviewActive),
      materializedProjectionGraph: graph,
    })),
  setAgentStatus: (status: AgentStatusEvent | null) =>
    set((state) => ({
      ...(status ? exitSamplePreviewState(state.samplePreviewActive) : {}),
      agentStatus: status,
    })),
  upsertLiveAssistCard: (card: LiveAssistCardRecord) =>
    set((state) => ({
      ...exitSamplePreviewState(state.samplePreviewActive),
      liveAssistCards: upsertLiveAssistCardRecord(
        state.samplePreviewActive ? [] : state.liveAssistCards,
        card,
      ),
      agentProposals:
        card.status === "pending"
          ? upsertAgentProposal(
              state.samplePreviewActive ? [] : state.agentProposals,
              card.proposal,
            )
          : (state.samplePreviewActive ? [] : state.agentProposals).filter(
              (proposal) => proposal.id !== card.proposal.id,
            ),
      approvingAgentProposalIds:
        card.status === "pending"
          ? state.samplePreviewActive
            ? []
            : state.approvingAgentProposalIds
          : (state.samplePreviewActive
              ? []
              : state.approvingAgentProposalIds
            ).filter((id) => id !== card.proposal.id),
    })),
  addAgentProposal: (proposal: AgentProposalEvent) => {
    set((state) => ({
      ...exitSamplePreviewState(state.samplePreviewActive),
      agentProposals: upsertAgentProposal(
        (state.samplePreviewActive ? [] : state.agentProposals).slice(-49),
        proposal,
      ),
    }));
    // Questions default to the graph: auto-record a Question node (local,
    // no LLM, never rate-limits). The card then only offers the OPTIONAL
    // "Ask AI" action to fetch a possible answer.
    if (proposal.kind === "question") {
      const text =
        proposal.body?.replace(
          /^Consider answering or linking this question:\s*/i,
          "",
        ) || proposal.title;
      void Promise.resolve(
        invoke("add_question_to_graph", {
          text,
          speaker: proposal.speaker_label ?? null,
          sourceSegmentId: proposal.source_segment_id ?? null,
        }),
      ).catch((err) =>
        console.error("auto add_question_to_graph failed:", err),
      );
    }
  },
  askAgentProposal: async (proposalId: string) => {
    const proposal = get().agentProposals.find((p) => p.id === proposalId);
    if (!proposal) return;
    const question =
      proposal.body?.replace(
        /^Consider answering or linking this question:\s*/i,
        "",
      ) || proposal.title;
    // Preserve the card as a dismissed live-assist record, then route the
    // question through the normal streaming chat (429-safe: errors surface
    // in the chat bubble rather than throwing).
    const dismissed = await get().dismissAgentProposal(proposalId);
    if (!dismissed) return;
    await get().sendChatMessage(question);
  },
  approveAgentProposal: async (proposalId: string) => {
    const proposal = get().agentProposals.find(
      (item) => item.id === proposalId,
    );
    if (!proposal) {
      set({ error: "Agent proposal no longer exists" });
      return null;
    }
    if (get().approvingAgentProposalIds.includes(proposalId)) {
      return null;
    }
    set((state) => ({
      approvingAgentProposalIds: [
        ...state.approvingAgentProposalIds,
        proposalId,
      ],
    }));
    try {
      const card = await invoke<LiveAssistCardRecord>(
        "approve_agent_proposal",
        { proposalId },
      );
      const result = card.outcome;
      if (!result) {
        throw new Error("Approved live assist card did not include an outcome");
      }
      const message: ChatMessage = {
        role: "assistant",
        content: result.message,
      };
      set((state) => ({
        liveAssistCards: upsertLiveAssistCardRecord(
          state.liveAssistCards,
          card,
        ),
        agentProposals: state.agentProposals.filter(
          (item) => item.id !== proposalId,
        ),
        approvingAgentProposalIds: state.approvingAgentProposalIds.filter(
          (id) => id !== proposalId,
        ),
        chatMessages: [...state.chatMessages, message],
        error: null,
      }));
      return result;
    } catch (e) {
      set((state) => ({
        approvingAgentProposalIds: state.approvingAgentProposalIds.filter(
          (id) => id !== proposalId,
        ),
        error: errorToMessage(e),
      }));
      return null;
    }
  },
  dismissAgentProposal: async (proposalId: string) => {
    if (get().approvingAgentProposalIds.includes(proposalId)) {
      return null;
    }
    try {
      const card = await invoke<LiveAssistCardRecord | null>(
        "dismiss_agent_proposal",
        { proposalId },
      );
      set((state) => ({
        liveAssistCards: card
          ? upsertLiveAssistCardRecord(state.liveAssistCards, card)
          : state.liveAssistCards,
        agentProposals: state.agentProposals.filter(
          (item) => item.id !== proposalId,
        ),
        approvingAgentProposalIds: state.approvingAgentProposalIds.filter(
          (id) => id !== proposalId,
        ),
      }));
      return card;
    } catch (err) {
      console.error("Failed to dismiss agent proposal:", err);
      set({ error: errorToMessage(err) });
      return null;
    }
  },
  clearAgentProposals: async () => {
    if (get().approvingAgentProposalIds.length > 0) {
      return [];
    }
    try {
      const cards = await invoke<LiveAssistCardRecord[]>(
        "clear_agent_proposals",
      );
      set((state) => ({
        liveAssistCards: cards.reduce(
          upsertLiveAssistCardRecord,
          state.liveAssistCards,
        ),
        agentProposals: [],
        approvingAgentProposalIds: [],
      }));
      return cards;
    } catch (err) {
      console.error("Failed to clear agent proposals:", err);
      set({ error: errorToMessage(err) });
      return [];
    }
  },
  clearTranscript: () =>
    set((state) =>
      state.samplePreviewActive
        ? clearSamplePreviewState()
        : {
            samplePreviewActive: false,
            transcriptSegments: [],
            asrPartial: null,
            asrSpanRevisions: [],
            diarizationSpanRevisions: [],
            sessionTranscriptEvents: [],
            sessionProjectionEvents: [],
            materializedNotes: null,
            materializedProjectionGraph: null,
            turnEvents: [],
            agentStatus: null,
            agentProposals: [],
            liveAssistCards: [],
            approvingAgentProposalIds: [],
          },
    ),
  loadSampleSessionPreview: (language?: string) =>
    set(sampleSessionPreviewState(language)),

  // ── Knowledge graph ──────────────────────────────────────────────────
  graphSnapshot: emptyGraphSnapshot(),
  setGraphSnapshot: (snapshot) =>
    set((state) => {
      // Preserve node object identity across snapshots. react-force-graph
      // stores each node's live simulation state (x/y/vx/vy/fx/fy) ON the
      // node object; if we hand it brand-new objects every GRAPH_UPDATE the
      // D3 force sim reheats and all nodes jump. Reuse the prior object for
      // any node whose id we already have (refreshing its data fields), so
      // the layout stays warm and stable.
      const prev = new Map(
        (state.samplePreviewActive ? [] : state.graphSnapshot.nodes).map(
          (n) => [n.id, n],
        ),
      );
      const nodes = snapshot.nodes.map((incoming) => {
        const existing = prev.get(incoming.id);
        return existing ? Object.assign(existing, incoming) : incoming;
      });
      // Seed positions for any node that doesn't have one yet (new nodes),
      // using already-positioned nodes from the merged set.
      const positioned = new Map(nodes.map((n) => [n.id, n]));
      seedNodePositions(nodes, snapshot.links, positioned);
      return {
        ...exitSamplePreviewState(state.samplePreviewActive),
        graphSnapshot: { ...snapshot, nodes },
      };
    }),
  applyGraphDelta: (delta: GraphDelta) =>
    set((state) => {
      const graphSnapshot = state.samplePreviewActive
        ? emptyGraphSnapshot()
        : state.graphSnapshot;
      const removedNodes = new Set(delta.removed_node_ids);
      const removedEdges = new Set(delta.removed_edge_ids);
      const prev = new Map(graphSnapshot.nodes.map((n) => [n.id, n]));
      const nodes = new Map<string, GraphNode>();

      for (const node of graphSnapshot.nodes) {
        if (!removedNodes.has(node.id)) {
          nodes.set(node.id, node);
        }
      }
      for (const node of delta.added_nodes) {
        nodes.set(node.id, node);
      }
      for (const node of delta.updated_nodes) {
        // Merge onto the existing object to keep its x/y; a fresh object
        // would reset the node to origin and reheat the layout.
        const existing = prev.get(node.id);
        nodes.set(node.id, existing ? Object.assign(existing, node) : node);
      }

      const nextNodes = [...nodes.values()];
      const links = new Map<string, GraphLink>();
      for (const link of graphSnapshot.links) {
        const source = graphEndpointId(link.source);
        const target = graphEndpointId(link.target);
        const id = graphLinkId(link);
        if (
          !removedEdges.has(id) &&
          !removedNodes.has(source) &&
          !removedNodes.has(target)
        ) {
          links.set(id, link);
        }
      }
      for (const edge of delta.added_edges) {
        links.set(edge.id, edge);
      }
      // Merge edge weight/label changes onto existing links by id (or add
      // them if we somehow don't have the edge yet), so edge strength
      // stays current between full snapshots.
      for (const edge of delta.updated_edges ?? []) {
        const existing = links.get(edge.id);
        links.set(edge.id, existing ? Object.assign(existing, edge) : edge);
      }

      const nextLinks = [...links.values()];
      seedNodePositions(
        nextNodes,
        nextLinks,
        nodes as Map<string, PositionedNode>,
      );
      return {
        ...exitSamplePreviewState(state.samplePreviewActive),
        graphSnapshot: {
          nodes: nextNodes,
          links: nextLinks,
          stats: {
            total_nodes: nextNodes.length,
            total_edges: nextLinks.length,
            total_episodes: graphSnapshot.stats.total_episodes,
          },
        },
      };
    }),

  // ── Exports ──────────────────────────────────────────────────────────
  exportTranscript: async () => {
    if (get().samplePreviewActive) {
      return JSON.stringify(
        {
          session_id: SAMPLE_SESSION_ID,
          preview: true,
          segments: get().transcriptSegments,
          events: get().sessionTranscriptEvents,
        },
        null,
        2,
      );
    }
    return await safeInvoke<string>("export_transcript");
  },
  exportGraph: async () => {
    if (get().samplePreviewActive) {
      return JSON.stringify(
        {
          session_id: SAMPLE_SESSION_ID,
          preview: true,
          materialized_graph: get().materializedProjectionGraph,
          snapshot: get().graphSnapshot,
        },
        null,
        2,
      );
    }
    return await safeInvoke<string>("export_graph");
  },
  getSessionId: async () => {
    if (get().samplePreviewActive) return SAMPLE_SESSION_ID;
    return await safeInvoke<string>("get_session_id");
  },

  // ── Pipeline status ──────────────────────────────────────────────────
  pipelineStatus: {
    capture: idleStage,
    pipeline: idleStage,
    asr: idleStage,
    diarization: idleStage,
    entity_extraction: idleStage,
    graph: idleStage,
  },
  setPipelineStatus: (status) => set({ pipelineStatus: status }),
  pipelineLatencies: {},
  setPipelineLatency: (sample: PipelineLatencyEvent) =>
    set((state) => ({
      pipelineLatencies: {
        ...state.pipelineLatencies,
        [sample.stage]: sample,
      },
    })),
  latestAudioConsumerHealth: null,
  setAudioConsumerHealth: (payload: ProcessedAudioConsumerHealthPayload) =>
    set({ latestAudioConsumerHealth: payload }),
  persistenceQueueBackpressure: {},
  setPersistenceQueueBackpressure: (payload) =>
    set((state) => {
      const next = { ...state.persistenceQueueBackpressure };
      if (payload.is_backpressured) {
        next[payload.writer] = payload;
      } else {
        delete next[payload.writer];
      }
      return { persistenceQueueBackpressure: next };
    }),

  // ── Speakers ─────────────────────────────────────────────────────────
  speakers: [],
  addOrUpdateSpeaker: (speaker) =>
    set((state) => {
      const existingSpeakers = state.samplePreviewActive ? [] : state.speakers;
      const idx = existingSpeakers.findIndex((s) => s.id === speaker.id);
      if (idx >= 0) {
        const updated = [...existingSpeakers];
        updated[idx] = speaker;
        return {
          ...exitSamplePreviewState(state.samplePreviewActive),
          speakers: updated,
        };
      }
      return {
        ...exitSamplePreviewState(state.samplePreviewActive),
        speakers: [...existingSpeakers, speaker],
      };
    }),
  clearSpeakers: () =>
    set((state) => ({
      ...exitSamplePreviewState(state.samplePreviewActive),
      speakers: [],
    })),

  // ── Capture state ────────────────────────────────────────────────────
  isCapturing: false,
  captureStartTime: null,
  setIsCapturing: (capturing) =>
    set((state) => ({
      ...(capturing ? exitSamplePreviewState(state.samplePreviewActive) : {}),
      isCapturing: capturing,
    })),
  backpressuredSources: [],
  setSourceBackpressure: (sourceId, isBackpressured) =>
    set((state) => {
      const present = state.backpressuredSources.includes(sourceId);
      if (isBackpressured && !present) {
        return {
          backpressuredSources: [...state.backpressuredSources, sourceId],
        };
      }
      if (!isBackpressured && present) {
        return {
          backpressuredSources: state.backpressuredSources.filter(
            (id) => id !== sourceId,
          ),
        };
      }
      return {};
    }),
  startCapture: async () => {
    const { selectedSourceIds, audioSources } = get();
    if (selectedSourceIds.length === 0) {
      set({ error: "No audio source selected" });
      return;
    }
    set((state) => ({
      ...exitSamplePreviewState(state.samplePreviewActive),
      // Starting a fresh live capture leaves any historical session view, so
      // the data-route report should follow the live session, not the old one.
      loadedSessionId: null,
    }));
    const sourcesBySelectionId = new Map<string, AudioSourceInfo>();
    for (const source of audioSources) {
      sourcesBySelectionId.set(source.id, source);
      if (source.capture_target)
        sourcesBySelectionId.set(source.capture_target, source);
    }
    const startedSourceIds: string[] = [];
    try {
      for (const sourceId of selectedSourceIds) {
        const source = sourcesBySelectionId.get(sourceId);
        await invoke(
          "start_capture",
          source ? { sourceId, source } : { sourceId },
        );
        startedSourceIds.push(sourceId);
      }
      set({
        isCapturing: true,
        captureStartTime: Date.now(),
        error: null,
      });
    } catch (e) {
      await Promise.allSettled(
        startedSourceIds.map((sourceId) =>
          invoke("stop_capture", { sourceId }),
        ),
      );
      set({ error: errorToMessage(e) });
    }
  },
  stopCapture: async () => {
    const { selectedSourceIds } = get();
    if (selectedSourceIds.length === 0) return;
    try {
      for (const sourceId of selectedSourceIds) {
        await invoke("stop_capture", { sourceId });
      }
      set({
        isCapturing: false,
        isTranscribing: false,
        isGeminiActive: false,
        activeGeminiCommand: null,
        captureStartTime: null,
        backpressuredSources: [],
        persistenceQueueBackpressure: {},
        error: null,
      });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },

  // ── Transcribe state ────────────────────────────────────────────────────────
  isTranscribing: false,
  startTranscribe: async () => {
    const { isCapturing } = get();
    if (!isCapturing) {
      set({ error: "Cannot start transcription: capture is not running" });
      return;
    }
    set((state) => exitSamplePreviewState(state.samplePreviewActive));
    try {
      await invoke("start_transcribe");
      set({
        isTranscribing: true,
        error: null,
      });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },
  stopTranscribe: async () => {
    try {
      await invoke("stop_transcribe");
      set({
        isTranscribing: false,
        error: null,
      });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },

  // ── Gemini Live dual pipeline ─────────────────────────────────────────
  isGeminiActive: false,
  geminiTranscripts: [],
  activeGeminiCommand: null,
  addGeminiTranscript: (entry: GeminiTranscriptEntry) =>
    set((state) => ({
      geminiTranscripts: [...state.geminiTranscripts.slice(-499), entry],
    })),
  clearGeminiTranscripts: () => set({ geminiTranscripts: [] }),
  startGemini: async () => {
    const {
      isCapturing,
      conversationMode,
      converseEngine,
      converseRealtimeAgentProvider,
    } = get();
    if (!isCapturing) {
      set({ error: "Cannot start Gemini: capture is not running" });
      return;
    }
    set((state) => exitSamplePreviewState(state.samplePreviewActive));
    // Route to the native speech-to-speech runtime when the user is in
    // Converse mode with the native engine; otherwise stay on the TEXT/notes
    // Gemini Live pipeline. Within native S2S the user picks the realtime-agent
    // provider (Gemini Live vs. the OpenAI Realtime voice agent). Remember which
    // command we started so `stopGemini` tears down the matching session.
    const nativeConverse =
      conversationMode === "converse" && converseEngine === "native";
    let startCommand:
      | "start_gemini"
      | "start_converse"
      | "start_openai_realtime";
    if (!nativeConverse) {
      startCommand = "start_gemini";
    } else if (converseRealtimeAgentProvider === "openai") {
      startCommand = "start_openai_realtime";
    } else {
      startCommand = "start_converse";
    }
    try {
      await invoke(startCommand);
      set({
        isGeminiActive: true,
        activeGeminiCommand: startCommand,
        error: null,
      });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },
  stopGemini: async () => {
    // Stop whichever session we started. When the tracked command is known,
    // stop exactly that one. When it's UNKNOWN (null) — e.g. state seeded
    // directly in tests, or lost across a reload/recovery while a converse
    // session is actually live — defaulting to stop_gemini would hit the
    // wrong backend and leave the converse session running (FINDING #57 P3).
    // Both stop commands are idempotent (stopping an inactive session is a
    // no-op), so fire BOTH defensively to guarantee teardown.
    const active = get().activeGeminiCommand;
    try {
      if (active === "start_converse") {
        await invoke("stop_converse");
      } else if (active === "start_openai_realtime") {
        await invoke("stop_openai_realtime");
      } else if (active === "start_gemini") {
        await invoke("stop_gemini");
      } else {
        // Unknown which session is live — tear down all. Use allSettled so one
        // backend rejecting (e.g. "not running") doesn't abort the others.
        const results = await Promise.allSettled([
          invoke("stop_converse"),
          invoke("stop_openai_realtime"),
          invoke("stop_gemini"),
        ]);
        const failure = results.find((r) => r.status === "rejected");
        if (failure && failure.status === "rejected") {
          throw failure.reason;
        }
      }
      set({
        isGeminiActive: false,
        activeGeminiCommand: null,
        error: null,
      });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },

  // ── Error state ──────────────────────────────────────────────────────
  error: null,
  setError: (error) => set({ error }),
  clearError: () => set({ error: null }),

  // ── Notifications (ADR-0011) ─────────────────────────────────────────
  // Unified transient feedback queue. Replaces the single-slot module
  // Toast: callers `notify(...)`, the <Notifications> host renders the
  // stack (newest last) above modals with severity-mapped aria-live.
  notifications: [],
  notify: ({ severity = "info", message, sticky, action, id }) => {
    const nid =
      id ?? `ntf-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    set((state) => ({
      notifications: [
        ...state.notifications,
        {
          id: nid,
          severity,
          message,
          sticky,
          action,
          createdAt: Date.now(),
        },
      ],
    }));
    return nid;
  },
  dismissNotification: (id) =>
    set((state) => ({
      notifications: state.notifications.filter((n) => n.id !== id),
    })),
  clearNotifications: () => set({ notifications: [] }),

  // ── Chat ─────────────────────────────────────────────────────────────
  chatMessages: [],
  isChatLoading: false,
  streamingChatRequestId: null,
  rightPanelTab: "transcript",
  setRightPanelTab: (tab) => set({ rightPanelTab: tab }),
  agentOverlayOpen: false,
  setAgentOverlayOpen: (open: boolean) => set({ agentOverlayOpen: open }),
  toggleAgentOverlay: () =>
    set((state) => ({ agentOverlayOpen: !state.agentOverlayOpen })),
  tokenOverlayOpen: false,
  setTokenOverlayOpen: (open: boolean) => set({ tokenOverlayOpen: open }),
  toggleTokenOverlay: () =>
    set((state) => ({ tokenOverlayOpen: !state.tokenOverlayOpen })),
  // Conversation mode: when false (default) the app uses the cascading
  // STT -> LLM -> TTS pipeline; when true the native speech-to-speech path
  // (Gemini Live / OpenAI realtime) is enabled and its top-bar control shows.
  nativeS2sEnabled: (() => {
    try {
      return localStorage.getItem("ag.nativeS2sEnabled") === "true";
    } catch {
      return false;
    }
  })(),
  setNativeS2sEnabled: (enabled: boolean) => {
    try {
      localStorage.setItem("ag.nativeS2sEnabled", String(enabled));
    } catch {
      /* ignore */
    }
    set({ nativeS2sEnabled: enabled });
  },

  // ── Theme (ADR-0009, Wave 4) ─────────────────────────────────────────
  // Persisted UI preference: "system" defers to prefers-color-scheme, while
  // "light"/"dark" pin the palette. The initial value is read from the same
  // localStorage key that main.tsx applied before first paint, so the store
  // and the DOM start in agreement. setTheme persists + reflects the choice
  // onto document.documentElement (see src/theme.ts).
  theme: readStoredTheme(),
  setTheme: (theme) => {
    persistTheme(theme);
    set({ theme });
  },

  // ── Conversation mode (ADR-0013) ─────────────────────────────────────
  // `notes`   → transcribe to build the knowledgebase (graph + notes).
  // `converse`→ talk *to* the knowledgebase. Engine: `pipelined`
  //   (STT → graph-grounded LLM → TTS, reuses the working chat + speak-aloud
  //   path) or `native` (Gemini Live; OpenAI Realtime later). Persisted and
  //   migrated from the legacy `nativeS2sEnabled` flag.
  conversationMode: (() => {
    try {
      const stored = localStorage.getItem("ag.conversationMode");
      if (stored === "notes" || stored === "converse") return stored;
      // Migrate: native-S2S users start in converse mode.
      return localStorage.getItem("ag.nativeS2sEnabled") === "true"
        ? "converse"
        : "notes";
    } catch {
      return "notes";
    }
  })(),
  setConversationMode: (mode) => {
    try {
      localStorage.setItem("ag.conversationMode", mode);
    } catch {
      /* ignore */
    }
    set({ conversationMode: mode });
  },
  converseEngine: (() => {
    try {
      const stored = localStorage.getItem("ag.converseEngine");
      if (stored === "native" || stored === "pipelined") return stored;
      return localStorage.getItem("ag.nativeS2sEnabled") === "true"
        ? "native"
        : "pipelined";
    } catch {
      return "pipelined";
    }
  })(),
  setConverseEngine: (engine) => {
    try {
      localStorage.setItem("ag.converseEngine", engine);
    } catch {
      /* ignore */
    }
    // Keep the legacy native-S2S flag in sync so the existing Gemini
    // start path and Settings checkbox stay consistent.
    const nativeOn =
      engine === "native" && get().conversationMode === "converse";
    try {
      localStorage.setItem("ag.nativeS2sEnabled", String(nativeOn));
    } catch {
      /* ignore */
    }
    set({ converseEngine: engine, nativeS2sEnabled: nativeOn });
  },
  converseRealtimeAgentProvider: (() => {
    try {
      const stored = localStorage.getItem("ag.converseRealtimeAgentProvider");
      if (stored === "gemini" || stored === "openai") return stored;
      return "gemini";
    } catch {
      return "gemini";
    }
  })(),
  setConverseRealtimeAgentProvider: (provider) => {
    try {
      localStorage.setItem("ag.converseRealtimeAgentProvider", provider);
    } catch {
      /* ignore */
    }
    set({ converseRealtimeAgentProvider: provider });
  },
  sendChatMessage: async (message: string) => {
    // Optimistic user message + empty assistant placeholder for the
    // streaming reply to grow into. Channel `delta` frames append onto the
    // placeholder; finalizeChatStream replaces its content with the
    // authoritative full_text from the `done` frame.
    const userMsg: ChatMessage = { role: "user", content: message };
    const assistantPlaceholder: ChatMessage = {
      role: "assistant",
      content: "",
    };
    set((state) => ({
      chatMessages: [...state.chatMessages, userMsg, assistantPlaceholder],
      isChatLoading: true,
    }));

    // Try streaming first (Api / OpenRouter providers). If the active
    // provider doesn't support streaming yet, fall back to the blocking
    // command — its channel-vs-promise contract is identical from the UI's
    // perspective: replace the placeholder with the final assistant message.
    //
    // audio-graph-1534: the per-token hot path is delivered over a
    // `tauri::ipc::Channel<ChatStreamEvent>` created here and passed as the
    // invoke arg, replacing the old `chat-token-delta` / `chat-token-done`
    // events. Because `onmessage` is wired BEFORE the invoke, no frame can be
    // lost between spawn and handler-registration — this removes the old
    // spawn-before-return early-delta race (and its module-scope buffer).
    //
    // `streamingChatRequestId` is only known once the invoke resolves. Frames
    // can arrive before that, so the handler coalesces deltas into a local
    // buffer and flushes them (at most once per CHAT_DELTA_THROTTLE_MS) once
    // the id is armed; a `done` frame drains synchronously before finalizing.
    let requestId: string | null = null;
    let doneEvent: ChatTokenDoneEvent | null = null;
    let pendingDelta = "";
    let latestFinishReason: string | undefined;
    let flushTimer: ReturnType<typeof setTimeout> | null = null;

    const flushDeltas = () => {
      flushTimer = null;
      if (requestId === null || pendingDelta.length === 0) return;
      const delta = pendingDelta;
      const finishReason = latestFinishReason;
      pendingDelta = "";
      latestFinishReason = undefined;
      get().appendChatTokenDelta({
        request_id: requestId,
        delta,
        finish_reason: finishReason,
      });
    };
    const scheduleFlush = () => {
      // Only start the timer once the id is armed; before that, deltas simply
      // accumulate in `pendingDelta` and are flushed by the drainNow() call
      // that runs the moment the invoke resolves and arms the id.
      if (requestId === null || flushTimer !== null) return;
      flushTimer = setTimeout(flushDeltas, CHAT_DELTA_THROTTLE_MS);
    };
    const drainNow = () => {
      if (flushTimer !== null) {
        clearTimeout(flushTimer);
        flushTimer = null;
      }
      flushDeltas();
    };
    const applyDone = () => {
      if (doneEvent === null) return;
      // Drain queued deltas first so the assistant message reflects everything
      // received before finalizing with the authoritative full_text.
      drainNow();
      get().finalizeChatStream(doneEvent);
    };

    const channel = new Channel<ChatStreamEvent>();
    channel.onmessage = (msg) => {
      if (msg.event === "delta") {
        pendingDelta += msg.data.delta;
        if (msg.data.finish_reason) latestFinishReason = msg.data.finish_reason;
        scheduleFlush();
      } else {
        // Terminal frame. If the invoke hasn't resolved yet (id not armed),
        // hold it — applyDone runs the moment the id is armed below.
        doneEvent = msg.data;
        if (requestId !== null) applyDone();
      }
    };

    try {
      requestId = await invoke<string>("start_streaming_chat", {
        message,
        channel,
      });
      set({ streamingChatRequestId: requestId });
      // Id armed: flush any deltas that arrived before it, then apply a
      // terminal frame if one already landed (done-before-resolve ordering).
      drainNow();
      if (doneEvent !== null) applyDone();
      // The channel handler above drives the rest of the stream, routing
      // frames into the placeholder we just inserted.
      return;
    } catch (streamErr) {
      // Streaming failed (most likely: provider doesn't support it).
      // Fall through to the legacy blocking path. Tear down the coalescer so
      // a stray frame can't touch the store after we've switched paths.
      if (flushTimer !== null) {
        clearTimeout(flushTimer);
        flushTimer = null;
      }
      channel.onmessage = () => {};
      console.info(
        "Streaming chat unavailable; using blocking path:",
        streamErr,
      );
    }

    try {
      const response = await invoke<ChatResponse>("send_chat_message", {
        message,
      });
      set((state) => {
        // Replace the empty placeholder (last message) with the
        // real assistant message. If the placeholder is no longer
        // last (concurrent agent proposal etc.), append.
        const last = state.chatMessages[state.chatMessages.length - 1];
        const isPlaceholder =
          last && last.role === "assistant" && last.content === "";
        const trimmed = isPlaceholder
          ? state.chatMessages.slice(0, -1)
          : state.chatMessages;
        return {
          chatMessages: [...trimmed, response.message],
          isChatLoading: false,
          streamingChatRequestId: null,
        };
      });
    } catch (e) {
      const errorMsg: ChatMessage = {
        role: "assistant",
        content: `Error: ${errorToMessage(e)}`,
      };
      set((state) => {
        const last = state.chatMessages[state.chatMessages.length - 1];
        const isPlaceholder =
          last && last.role === "assistant" && last.content === "";
        const trimmed = isPlaceholder
          ? state.chatMessages.slice(0, -1)
          : state.chatMessages;
        return {
          chatMessages: [...trimmed, errorMsg],
          isChatLoading: false,
          streamingChatRequestId: null,
        };
      });
    }
  },
  appendChatTokenDelta: (event: ChatTokenDeltaEvent) => {
    // Only apply deltas for the currently-tracked request id. The
    // channel-based sender in sendChatMessage only calls this once the id is
    // armed (it coalesces + holds leading frames until then), so a null or
    // mismatched id here means the stream is stale — e.g. a delta racing a
    // clearChatHistory, or a second stream started while the first drained.
    // Drop it: there's no placeholder this delta legitimately belongs to.
    const current = get().streamingChatRequestId;
    if (current === null || current !== event.request_id) {
      return;
    }
    set((state) => {
      if (state.chatMessages.length === 0) return {};
      const last = state.chatMessages[state.chatMessages.length - 1];
      if (last.role !== "assistant") return {};
      const updated: ChatMessage = {
        ...last,
        content: last.content + event.delta,
      };
      return {
        chatMessages: [...state.chatMessages.slice(0, -1), updated],
      };
    });
  },
  finalizeChatStream: (event: ChatTokenDoneEvent) => {
    const current = get().streamingChatRequestId;
    if (current !== null && current !== event.request_id) {
      return;
    }
    set((state) => {
      if (state.chatMessages.length === 0) {
        return {
          isChatLoading: false,
          streamingChatRequestId: null,
        };
      }
      const last = state.chatMessages[state.chatMessages.length - 1];
      if (last.role !== "assistant") {
        return {
          isChatLoading: false,
          streamingChatRequestId: null,
        };
      }
      // Authoritative: replace whatever the deltas accumulated with
      // the final full_text. Handles three cases:
      //   1. The provider revised the reply on the terminal chunk.
      //   2. We dropped a delta event (network glitch, IPC backlog).
      //   3. The stream terminated with an error (e.g. HTTP 429 rate
      //      limit) — surface it in the bubble instead of leaving it
      //      blank, which previously looked like a silent hang.
      let finalContent: string;
      if (event.finish_reason?.startsWith("error:")) {
        const detail = event.finish_reason.slice("error:".length).trim();
        const friendly = /429|rate.?limit|too many requests/i.test(detail)
          ? "Rate limited by the model provider (HTTP 429). The free tier is capped — switch to a non-`:free` OpenRouter model or add credits, then try again."
          : detail || "the request failed";
        finalContent =
          (event.full_text ? `${event.full_text}\n\n` : "") +
          `⚠️ Chat failed: ${friendly}`;
      } else if (
        event.finish_reason === "cancelled" &&
        event.full_text === ""
      ) {
        finalContent = `${last.content} [cancelled]`;
      } else {
        finalContent = event.full_text;
      }
      const updated: ChatMessage = { ...last, content: finalContent };
      return {
        chatMessages: [...state.chatMessages.slice(0, -1), updated],
        isChatLoading: false,
        streamingChatRequestId: null,
      };
    });
  },
  clearChatHistory: async () => {
    try {
      await invoke("clear_chat_history");
      set({ chatMessages: [], streamingChatRequestId: null });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },

  // ── Models ────────────────────────────────────────────────────────────
  models: [],
  isDownloading: false,
  downloadProgress: null,
  fetchModels: async () => {
    try {
      const models = await invoke<ModelInfo[]>("list_available_models");
      set({ models, error: null });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },
  downloadModel: async (filename: string) => {
    set({ isDownloading: true, downloadProgress: null });
    try {
      await invoke("download_model_cmd", { modelFilename: filename });
      // Refresh model list after download
      const models = await invoke<ModelInfo[]>("list_available_models");
      set({ models, isDownloading: false, error: null });
    } catch (e) {
      set({
        isDownloading: false,
        error: errorToMessage(e),
      });
    }
  },

  // ── API endpoint ──────────────────────────────────────────────────────
  apiConfig: null,
  configureApiEndpoint: async (config: ApiEndpointConfig) => {
    try {
      await invoke("configure_api_endpoint", {
        endpoint: config.endpoint,
        apiKey: config.apiKey ?? null,
        model: config.model,
      });
      set({ apiConfig: config, error: null });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },
  clearApiEndpoint: () => set({ apiConfig: null }),

  // ── Settings ──────────────────────────────────────────────────────────
  settings: null,
  // `undefined` = not toggled this session; footer Save defers to the loaded
  // `settings.analytics_enabled`. The Logging panel toggle sets an explicit
  // boolean here (without touching `settings` identity) once used.
  analyticsEnabled: undefined,
  modelStatus: null,
  settingsOpen: false,
  settingsLoading: false,
  isDeletingModel: null,

  openSettings: () => {
    set({ settingsOpen: true });
    const { fetchSettings, fetchModels, fetchModelStatus } = get();
    fetchSettings();
    fetchModels();
    fetchModelStatus();
  },
  closeSettings: () => set({ settingsOpen: false }),

  fetchSettings: async () => {
    set({ settingsLoading: true });
    try {
      const settings = await invoke<AppSettings>("load_settings_cmd");
      set({ settings, settingsLoading: false, error: null });
    } catch (e) {
      set({
        settingsLoading: false,
        error: errorToMessage(e),
      });
    }
  },
  saveSettings: async (settings: AppSettings) => {
    try {
      await invoke("save_settings_cmd", { settings });
      const redactedSettings = await invoke<AppSettings>("load_settings_cmd");
      set({ settings: redactedSettings, error: null });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },
  fetchModelStatus: async () => {
    try {
      const modelStatus = await invoke<ModelStatus>("get_model_status");
      set({ modelStatus, error: null });
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },
  deleteModel: async (filename: string) => {
    set({ isDeletingModel: filename });
    try {
      await invoke("delete_model_cmd", { modelFilename: filename });
      // Refresh models and model status after deletion
      const models = await invoke<ModelInfo[]>("list_available_models");
      const modelStatus = await invoke<ModelStatus>("get_model_status");
      set({ models, modelStatus, isDeletingModel: null, error: null });
    } catch (e) {
      set({
        isDeletingModel: null,
        error: errorToMessage(e),
      });
    }
  },

  // ── Credentials ───────────────────────────────────────────────────────
  saveCredential: async (key: string, value: string) => {
    await invoke("save_credential_cmd", { key, value });
  },
  deleteCredential: async (key: string) => {
    await invoke("delete_credential_cmd", { key });
  },

  // ── AWS profile discovery ─────────────────────────────────────────────
  listAwsProfiles: async () => {
    try {
      return await invoke<string[]>("list_aws_profiles");
    } catch (e) {
      console.error("Failed to list AWS profiles:", e);
      return [];
    }
  },

  // ── Sessions (v2) ─────────────────────────────────────────────────────
  sessionsBrowserOpen: false,
  sessions: [],
  sessionsLoading: false,
  loadedSessionId: null,
  openSessionsBrowser: () => {
    set({ sessionsBrowserOpen: true });
    const { listSessions, purgeExpiredSessions } = get();
    // Lazy cleanup of expired trash on every open. Fire-and-forget —
    // purge failures must not block the browser from rendering.
    void purgeExpiredSessions().catch(() => {});
    // Fetch fresh on open; ignore errors (handled inside listSessions).
    // Larger limit (200) than v1's 10 — the browser has its own search/
    // sort UI, so a bigger pool makes filtering meaningful.
    void listSessions(200).catch(() => {});
  },
  closeSessionsBrowser: () => set({ sessionsBrowserOpen: false }),
  listSessions: async (limit?: number) => {
    set({ sessionsLoading: true });
    try {
      const sessions = await invoke<SessionMetadata[]>("list_sessions", {
        limit: limit ?? null,
      });
      set({ sessions, sessionsLoading: false, error: null });
      return sessions;
    } catch (e) {
      set({
        sessionsLoading: false,
        error: errorToMessage(e),
      });
      return [];
    }
  },
  loadSessionTranscript: async (sessionId: string) => {
    try {
      const segments = await invoke<TranscriptSegment[]>(
        "load_session_transcript",
        { sessionId },
      );
      // Replace current transcript view with the loaded session's segments.
      set((state) => ({
        ...exitSamplePreviewState(state.samplePreviewActive),
        transcriptSegments: segments,
        asrPartial: null,
        asrSpanRevisions: [],
        sessionTranscriptEvents: [],
        sessionProjectionEvents: [],
        materializedNotes: null,
        materializedProjectionGraph: null,
        agentProposals: [],
        liveAssistCards: [],
        approvingAgentProposalIds: [],
        error: null,
      }));
      return segments;
    } catch (e) {
      set({ error: errorToMessage(e) });
      return [];
    }
  },
  loadSession: async (sessionId: string) => {
    try {
      const loaded = await invoke<LoadedSession>("load_session", { sessionId });
      set((state) => ({
        ...exitSamplePreviewState(state.samplePreviewActive),
        transcriptSegments: loaded.transcript,
        graphSnapshot: loaded.graph,
        asrPartial: null,
        asrSpanRevisions: [],
        sessionTranscriptEvents: loaded.transcript_events ?? [],
        sessionProjectionEvents: loaded.projection_events ?? [],
        materializedNotes: loaded.notes ?? null,
        materializedProjectionGraph: loaded.materialized_graph ?? null,
        liveAssistCards: loaded.live_assist_cards ?? [],
        agentProposals: [],
        approvingAgentProposalIds: [],
        loadedSessionId: sessionId,
        error: null,
      }));
      return loaded;
    } catch (e) {
      set({ error: errorToMessage(e) });
      return null;
    }
  },
  // Soft-delete: flips `deleted = true` in the index; files stay on disk.
  deleteSession: async (sessionId: string) => {
    try {
      await invoke("delete_session", { sessionId });
      set((state) => ({
        sessions: state.sessions.map((s) =>
          s.id === sessionId
            ? { ...s, deleted: true, deleted_at: Date.now() }
            : s,
        ),
        error: null,
      }));
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },
  restoreSession: async (sessionId: string) => {
    try {
      await invoke("restore_session", { sessionId });
      set((state) => ({
        sessions: state.sessions.map((s) =>
          s.id === sessionId ? { ...s, deleted: false, deleted_at: null } : s,
        ),
        error: null,
      }));
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },
  deleteSessionPermanently: async (sessionId: string) => {
    try {
      await invoke("delete_session_permanently", { sessionId });
      set((state) => ({
        sessions: state.sessions.filter((s) => s.id !== sessionId),
        error: null,
      }));
    } catch (e) {
      set({ error: errorToMessage(e) });
    }
  },
  purgeExpiredSessions: async () => {
    try {
      const purged = await invoke<string[]>("purge_expired_sessions");
      if (purged.length > 0) {
        set((state) => ({
          sessions: state.sessions.filter((s) => !purged.includes(s.id)),
        }));
      }
      return purged;
    } catch (e) {
      // Purge is best-effort housekeeping; don't stomp error state
      // because the user didn't initiate it explicitly.
      console.warn("purge_expired_sessions failed:", e);
      return [];
    }
  },
  recoverOrphanedSessions: async () => {
    try {
      const report = await invoke<SessionRecoveryReport>(
        "recover_orphaned_sessions",
      );
      const sessions = await get().listSessions(200);
      set({
        sessions,
        error:
          report.errors.length > 0
            ? `Recovered ${report.recovered} session(s); ${report.errors.length} file(s) had recoverable errors.`
            : null,
      });
      return report;
    } catch (e) {
      set({ error: errorToMessage(e) });
      return null;
    }
  },
  exportSessionBundle: async (sessionId: string) => {
    try {
      const bundle = await invoke<SessionExportBundle>(
        "export_session_bundle",
        {
          sessionId,
        },
      );
      set({ error: null });
      return bundle;
    } catch (e) {
      set({ error: errorToMessage(e) });
      return null;
    }
  },
}));
