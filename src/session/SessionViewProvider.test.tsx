import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "../store";
import type { GraphSnapshot, TranscriptSegment } from "../types";
import { SessionViewProvider, useSessionView } from "./SessionViewProvider";

const FAKE_GRAPH: GraphSnapshot = {
  nodes: [
    {
      id: "n1",
      name: "Node 1",
      entity_type: "person",
      val: 1,
      color: "#000000",
      first_seen: 0,
      last_seen: 0,
      mention_count: 1,
    },
  ],
  links: [],
  stats: { total_nodes: 1, total_edges: 0, total_episodes: 0 },
};

const FAKE_SEGMENT = {
  id: "seg-1",
  text: "hello",
} as unknown as TranscriptSegment;

describe("session: SessionViewProvider / useSessionView", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      transcriptSegments: [],
      graphSnapshot: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
      materializedNotes: null,
      sessionTimeline: null,
      sessionProjectionEvents: [],
    });
  });

  it("unwrapped: falls back to the global store's live values", () => {
    useAudioGraphStore.setState({
      transcriptSegments: [FAKE_SEGMENT],
      graphSnapshot: FAKE_GRAPH,
    });
    const { result } = renderHook(() => useSessionView());
    expect(result.current.transcriptSegments).toEqual([FAKE_SEGMENT]);
    expect(result.current.graphSnapshot).toEqual(FAKE_GRAPH);
    expect(result.current.materializedNotes).toBeNull();
    expect(result.current.sessionTimeline).toBeNull();
    expect(result.current.sessionProjectionEvents).toEqual([]);
  });

  it("wrapped: reads the same live values through the provider's context", () => {
    useAudioGraphStore.setState({
      transcriptSegments: [FAKE_SEGMENT],
      graphSnapshot: FAKE_GRAPH,
    });
    const { result } = renderHook(() => useSessionView(), {
      wrapper: ({ children }) => (
        <SessionViewProvider>{children}</SessionViewProvider>
      ),
    });
    expect(result.current.transcriptSegments).toEqual([FAKE_SEGMENT]);
    expect(result.current.graphSnapshot).toEqual(FAKE_GRAPH);
  });

  it("wrapped and unwrapped agree on identical live state (zero-behavior-change shim)", () => {
    useAudioGraphStore.setState({
      transcriptSegments: [FAKE_SEGMENT],
      graphSnapshot: FAKE_GRAPH,
      sessionProjectionEvents: [{ type: "notes_patch" } as never],
    });

    const unwrapped = renderHook(() => useSessionView());
    const wrapped = renderHook(() => useSessionView(), {
      wrapper: ({ children }) => (
        <SessionViewProvider>{children}</SessionViewProvider>
      ),
    });

    expect(wrapped.result.current).toEqual(unwrapped.result.current);
  });

  it("wrapped: re-renders with updated values after a store write", () => {
    const { result, rerender } = renderHook(() => useSessionView(), {
      wrapper: ({ children }) => (
        <SessionViewProvider>{children}</SessionViewProvider>
      ),
    });
    expect(result.current.transcriptSegments).toEqual([]);

    useAudioGraphStore.setState({ transcriptSegments: [FAKE_SEGMENT] });
    rerender();

    expect(result.current.transcriptSegments).toEqual([FAKE_SEGMENT]);
  });
});
